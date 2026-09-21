//! Hardened execution runner for untrusted repository commands and patch operations.
//!
//! This is the only place in the workspace that spawns a command on behalf of a
//! model. It decides isolation before it starts anything, records what it
//! actually applied, and terminates the whole process tree when a command runs
//! out of time.

pub mod fake;
pub mod journal;
pub mod tree;

use async_trait::async_trait;
pub use fake::{FakeRunner, FaultInjectionMode, RunnerFault};
pub use journal::{ActionJournal, ActionState, JournalEntry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use vitna_sandbox::{plan, SandboxConfig, SandboxGuarantee};

/// Prefix on the error returned when a command was refused for want of a
/// sandbox. Callers match on it to tell "we would not run this" apart from "it
/// ran and failed".
pub const SANDBOX_UNAVAILABLE: &str = "sandbox unavailable";

/// Recorded when a real process ran with no sandbox around it.
pub const BACKEND_NONE: &str = "none";

/// Recorded by a runner that starts no process at all.
///
/// Distinct from [`BACKEND_NONE`] on purpose. "Nothing confined this process"
/// and "there was no process" are different claims, and collapsing them would
/// make a fake runner's output look like host execution with the sandbox off.
pub const BACKEND_NO_PROCESS: &str = "no_process";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub statement_digest: String,
    /// The mechanism that confined this command, or `none`.
    pub sandbox_backend: String,
    /// `fully_enforced`, `fallback`, or `none`.
    pub sandbox_enforcement: String,
}

/// One command to run, and the authority under which to run it.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub command: String,
    pub working_dir: PathBuf,
    /// The root the sandbox makes writable. Distinct from `working_dir`, which
    /// may be a subdirectory of it.
    pub workspace_root: PathBuf,
    pub timeout_ms: u64,
    /// Explicit operator approval to run with no OS sandbox. Never defaulted to
    /// true, and never inferred from a general auto-approve setting: those two
    /// decisions are not the same decision.
    pub allow_unsandboxed: bool,
    pub allow_network: bool,
}

impl CommandRequest {
    pub fn new<P: AsRef<Path>, Q: AsRef<Path>>(
        command: impl Into<String>,
        working_dir: P,
        workspace_root: Q,
        timeout_ms: u64,
    ) -> Self {
        Self {
            command: command.into(),
            working_dir: working_dir.as_ref().to_path_buf(),
            workspace_root: workspace_root.as_ref().to_path_buf(),
            timeout_ms,
            allow_unsandboxed: false,
            allow_network: false,
        }
    }
}

/// The statement digest binds one action to what it reported: the action id, the
/// command, its exit code, both output streams, and the isolation it ran under.
/// Both runners compute it here so a receipt cannot depend on which runner
/// produced it, and the isolation is inside the digest so the claim cannot be
/// edited without breaking it.
#[allow(clippy::too_many_arguments)]
pub fn compute_statement_digest(
    action_id: &str,
    command: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
    sandbox_backend: &str,
    sandbox_enforcement: &str,
) -> String {
    let raw = format!(
        "statement:{}:{}:{}:{}:{}:{}:{}",
        action_id, command, exit_code, stdout, stderr, sandbox_backend, sandbox_enforcement
    );
    hex::encode(Sha256::digest(raw.as_bytes()))
}

/// What isolation this machine can provide for `workspace_root` right now.
///
/// Exposed so a policy layer can ask before it approves, rather than
/// discovering the answer from a failed execution.
pub fn sandbox_status(
    workspace_root: &Path,
    allow_network: bool,
) -> Result<(String, String), String> {
    let mut config = SandboxConfig::new(workspace_root, SandboxGuarantee::Guarded);
    config.allow_network = allow_network;
    match plan(&config, "true", workspace_root) {
        Ok(p) => Ok((
            p.backend.as_str().to_string(),
            p.enforcement.as_str().to_string(),
        )),
        Err(e) => Err(e.reason().to_string()),
    }
}

#[async_trait]
pub trait Runner: Send + Sync {
    async fn run_command(&self, request: CommandRequest) -> Result<ExecutionOutput, String>;

    /// Whether this runner starts real operating system processes.
    ///
    /// A policy layer asks before it decides whether a missing sandbox should
    /// block the action: a runner that fabricates output has nothing to
    /// contain, and refusing it would block work that never leaves this
    /// process. The default is true, so a new runner has to opt out
    /// deliberately rather than inherit an exemption by forgetting.
    fn spawns_processes(&self) -> bool {
        true
    }
}

/// Hardened process runner executing child processes with resource bounds and action journaling.
pub struct ProcessRunner {
    pub journal: Arc<Mutex<ActionJournal>>,
}

impl ProcessRunner {
    pub fn new(journal: ActionJournal) -> Self {
        Self {
            journal: Arc::new(Mutex::new(journal)),
        }
    }

    pub fn open_or_create<P: AsRef<Path>>(journal_path: P) -> std::io::Result<Self> {
        let journal = ActionJournal::open(journal_path)?;
        Ok(Self::new(journal))
    }
}

/// Environment handed to the child, after clearing everything else.
fn base_environment(extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        env.push(("PATH".to_string(), path));
    }
    #[cfg(windows)]
    {
        // Without these, much of the Win32 surface fails in ways that look like
        // the command being broken rather than the environment being empty.
        for key in ["SystemRoot", "SystemDrive", "COMSPEC", "PATHEXT"] {
            if let Ok(v) = std::env::var(key) {
                env.push((key.to_string(), v));
            }
        }
    }
    env.extend(extra.iter().cloned());
    env
}

#[async_trait]
impl Runner for ProcessRunner {
    async fn run_command(&self, request: CommandRequest) -> Result<ExecutionOutput, String> {
        let action_id = format!(
            "act-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let argument_digest = hex::encode(Sha256::digest(request.command.as_bytes()));
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Step 2: Record prepared in journal
        {
            let mut j = self.journal.lock().map_err(|e| e.to_string())?;
            j.record_prepared(&action_id, &argument_digest, now_ms)
                .map_err(|e| format!("Journal error on prepared: {}", e))?;
        }

        // Isolation is decided before anything starts. A command that cannot be
        // sandboxed is refused here unless the operator approved running without
        // one, and that refusal is recorded rather than silently downgraded.
        let mut config = SandboxConfig::new(&request.workspace_root, SandboxGuarantee::Guarded);
        config.allow_network = request.allow_network;

        let planned = plan(&config, &request.command, &request.working_dir);

        let (program, args, plan_env, backend, enforcement, namespaced) = match planned {
            Ok(p) => {
                let namespaced = matches!(p.backend, vitna_sandbox::SandboxBackend::Bubblewrap);
                (
                    p.program,
                    p.args,
                    p.env,
                    p.backend.as_str().to_string(),
                    p.enforcement.as_str().to_string(),
                    namespaced,
                )
            }
            Err(e) => {
                if !request.allow_unsandboxed {
                    let mut j = self.journal.lock().map_err(|e| e.to_string())?;
                    j.record_refused(&action_id, "sandbox_unavailable", now_ms)
                        .map_err(|e| format!("Journal error on refused: {}", e))?;
                    return Err(format!(
                        "{}: {}. Running this command without an OS sandbox needs explicit \
                         approval; a general auto-approve does not grant it.",
                        SANDBOX_UNAVAILABLE,
                        e.reason()
                    ));
                }

                // Explicit override. No VITNA_SANDBOX is set on this child,
                // because nothing is containing it.
                let (shell, flag) = if cfg!(windows) {
                    ("cmd.exe", "/C")
                } else {
                    ("/bin/sh", "-c")
                };
                (
                    shell.to_string(),
                    vec![flag.to_string(), request.command.clone()],
                    Vec::new(),
                    BACKEND_NONE.to_string(),
                    BACKEND_NONE.to_string(),
                    false,
                )
            }
        };

        // Step 3: Record started, with the isolation that is about to apply.
        {
            let mut j = self.journal.lock().map_err(|e| e.to_string())?;
            j.record_started(&action_id, now_ms)
                .map_err(|e| format!("Journal error on started: {}", e))?;
            j.record_sandbox(&action_id, &backend, &enforcement)
                .map_err(|e| format!("Journal error on sandbox: {}", e))?;
        }

        let start_time = Instant::now();

        let mut cmd = tokio::process::Command::new(&program);

        #[cfg(windows)]
        {
            // `cmd.exe /C <command>` needs the command verbatim. Rust's normal
            // argument quoting escapes the quotes inside it, and cmd then sees
            // `\"path\"` and reports that it is not a recognized command, so any
            // command containing a quoted path failed before it ran.
            use std::os::windows::process::CommandExt;
            let std_cmd = cmd.as_std_mut();
            for arg in &args {
                std_cmd.raw_arg(arg);
            }
        }
        #[cfg(not(windows))]
        cmd.args(&args);

        cmd.current_dir(&request.working_dir);
        cmd.env_clear();
        for (k, v) in base_environment(&plan_env) {
            cmd.env(k, v);
        }
        if backend != BACKEND_NONE {
            // Set only by the code that actually applied a backend, and set to
            // that backend's name. It used to be a hardcoded "1" on a child
            // nothing was containing.
            cmd.env("VITNA_SANDBOX", &backend);
            cmd.env("VITNA_SANDBOX_ENFORCEMENT", &enforcement);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        tree::configure(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let mut j = self.journal.lock().map_err(|e| e.to_string())?;
                j.record_refused(&action_id, "spawn_failed", now_ms)
                    .map_err(|e| format!("Journal error on spawn failure: {}", e))?;
                return Err(format!("Command spawn failed: {}", e));
            }
        };

        let guard = tree::attach(&child);

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(pipe) = stdout_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf).await;
            }
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(pipe) = stderr_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf).await;
            }
            buf
        });

        let timeout_dur = Duration::from_millis(request.timeout_ms);
        let wait_result = tokio::time::timeout(timeout_dur, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                let teardown = tree::kill_tree(&mut child, &guard, namespaced);
                let mut j = self.journal.lock().map_err(|e| e.to_string())?;
                j.record_interrupted(&action_id, "wait_failed", teardown.as_str(), now_ms)
                    .map_err(|e| format!("Journal error on wait failure: {}", e))?;
                return Err(format!("Command wait failed: {}", e));
            }
            Err(_) => {
                // The process is still running. Take the tree down before
                // reporting anything, then record what the teardown achieved.
                let teardown = tree::kill_tree(&mut child, &guard, namespaced);
                let _ = child.wait().await;

                let duration_ms = start_time.elapsed().as_millis() as u64;
                {
                    let mut j = self.journal.lock().map_err(|e| e.to_string())?;
                    j.record_interrupted(
                        &action_id,
                        "timeout",
                        teardown.as_str(),
                        now_ms + duration_ms,
                    )
                    .map_err(|e| format!("Journal error on timeout: {}", e))?;
                }

                return Err(format!(
                    "Command timed out after {} ms ({})",
                    request.timeout_ms,
                    teardown.as_str()
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&stdout_task.await.unwrap_or_default()).to_string();
        let stderr = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).to_string();

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let exit_code = status.code().unwrap_or(-1);

        let statement_digest = compute_statement_digest(
            &action_id,
            &request.command,
            exit_code,
            &stdout,
            &stderr,
            &backend,
            &enforcement,
        );

        // Step 4: Record finished in journal
        {
            let mut j = self.journal.lock().map_err(|e| e.to_string())?;
            j.record_finished(
                &action_id,
                exit_code,
                hex::encode(Sha256::digest(stdout.as_bytes())),
                hex::encode(Sha256::digest(stderr.as_bytes())),
                now_ms + duration_ms,
            )
            .map_err(|e| format!("Journal error on finished: {}", e))?;
        }

        // Step 5: Record acknowledged in journal
        {
            let mut j = self.journal.lock().map_err(|e| e.to_string())?;
            j.record_acknowledged(&action_id)
                .map_err(|e| format!("Journal error on acknowledged: {}", e))?;
        }

        Ok(ExecutionOutput {
            exit_code,
            stdout,
            stderr,
            duration_ms,
            statement_digest,
            sandbox_backend: backend,
            sandbox_enforcement: enforcement,
        })
    }
}
