//! Hardened execution runner for untrusted repository commands and patch operations.

pub mod fake;
pub mod journal;

use async_trait::async_trait;
pub use fake::{FakeRunner, FaultInjectionMode, RunnerFault};
pub use journal::{ActionJournal, ActionState, JournalEntry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub statement_digest: String,
}

/// The statement digest binds one action to what it reported: the action id, the
/// command, its exit code and both output streams. Both runners compute it here so
/// a receipt cannot depend on which runner produced it.
pub fn compute_statement_digest(
    action_id: &str,
    command: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) -> String {
    let raw = format!(
        "statement:{}:{}:{}:{}:{}",
        action_id, command, exit_code, stdout, stderr
    );
    hex::encode(Sha256::digest(raw.as_bytes()))
}

#[async_trait]
pub trait Runner: Send + Sync {
    async fn run_command(
        &self,
        command: &str,
        working_dir: &Path,
        timeout_ms: u64,
    ) -> Result<ExecutionOutput, String>;
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

#[async_trait]
impl Runner for ProcessRunner {
    async fn run_command(
        &self,
        command: &str,
        working_dir: &Path,
        timeout_ms: u64,
    ) -> Result<ExecutionOutput, String> {
        let action_id = format!(
            "act-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let argument_digest = hex::encode(Sha256::digest(command.as_bytes()));
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

        // Step 3: Record started in journal
        {
            let mut j = self.journal.lock().map_err(|e| e.to_string())?;
            j.record_started(&action_id, now_ms)
                .map_err(|e| format!("Journal error on started: {}", e))?;
        }

        let start_time = Instant::now();

        // Platform command invocation
        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd.exe");
            c.arg("/C").arg(command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(command);
            c
        };

        cmd.current_dir(working_dir);

        // Sanitize environment: strip ambient keys
        cmd.env_clear();
        cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");
        cmd.env("VITNA_SANDBOX", "1");

        let timeout_dur = Duration::from_millis(timeout_ms);
        let output_future = cmd.output();

        let output = match tokio::time::timeout(timeout_dur, output_future).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(format!("Command spawn failed: {}", e)),
            Err(_) => return Err(format!("Command timed out after {} ms", timeout_ms)),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let statement_digest =
            compute_statement_digest(&action_id, command, exit_code, &stdout, &stderr);

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
        })
    }
}