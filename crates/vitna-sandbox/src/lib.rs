//! Platform sandbox policy: what isolation was asked for, what this machine can
//! actually provide, and the exact invocation that provides it.
//!
//! This crate decides and describes. It does not spawn. `vitna-runner` is the
//! only spawner, so there is one timeout, one process-tree teardown, and one
//! place that can claim a command was sandboxed. Until this was split out there
//! were two spawn paths and both set `VITNA_SANDBOX=1` on a child that nothing
//! was containing.
//!
//! The vocabulary is `docs/PLATFORM_MATRIX.md`, which is the authority:
//!
//! - **Read-only**: no writes to the workspace.
//! - **Guarded**: native OS sandbox. Bubblewrap on Linux, Seatbelt on macOS,
//!   AppContainer on Windows.
//! - **Strong**: ephemeral container or hardware VM. Not implemented anywhere
//!   in this codebase, and asking for it returns an error rather than being
//!   quietly served by something weaker.
//! - **Full access**: host execution with no OS sandbox, only on explicit
//!   operator override, and per that document "never described or labeled as
//!   sandboxed in the UI or receipts".

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxGuarantee {
    ReadOnly,
    Guarded,
    Strong,
    FullAccess,
}

/// The mechanism that actually confines a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxBackend {
    Bubblewrap,
    Seatbelt,
    AppContainer,
}

impl SandboxBackend {
    /// The name recorded in the journal, the receipt, and `VITNA_SANDBOX`.
    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxBackend::Bubblewrap => "bubblewrap",
            SandboxBackend::Seatbelt => "seatbelt",
            SandboxBackend::AppContainer => "appcontainer",
        }
    }
}

/// How completely the backend delivered what the guarantee describes.
///
/// A backend that applied every restriction is `fully_enforced`. One that
/// applied some of them says `fallback` and names what is missing, because a
/// partial sandbox reported as a whole one is the failure this whole crate
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Enforcement {
    FullyEnforced,
    Fallback,
}

impl Enforcement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Enforcement::FullyEnforced => "fully_enforced",
            Enforcement::Fallback => "fallback",
        }
    }
}

/// Why no sandbox could be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// The platform has a mechanism but it is not usable here, or this build
    /// does not implement one. Carries a reason fit to show an operator.
    Unavailable { reason: String },
    /// A guarantee this codebase does not implement at all.
    NotImplemented { reason: String },
}

impl PlanError {
    pub fn reason(&self) -> &str {
        match self {
            PlanError::Unavailable { reason } => reason,
            PlanError::NotImplemented { reason } => reason,
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMount {
    pub host_path: PathBuf,
    pub sandbox_path: PathBuf,
    pub writable: bool,
}

/// A concrete invocation: exactly what to run, with what environment, to get the
/// isolation this plan claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    pub backend: SandboxBackend,
    pub enforcement: Enforcement,
    /// Present when `enforcement` is `Fallback`: what this backend could not do.
    pub gaps: Vec<String>,
    pub program: String,
    pub args: Vec<String>,
    /// Applied after clearing the environment.
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub workspace_root: PathBuf,
    pub mounts: Vec<SandboxMount>,
    pub allow_network: bool,
    pub max_memory_bytes: u64,
    pub max_cpu_percent: u32,
    pub max_processes: u32,
    pub sanitized_environment: Vec<(String, String)>,
    pub guarantee: SandboxGuarantee,
}

/// Paths inside the workspace that stay read-only even when the workspace is
/// writable.
///
/// A sandboxed process that can write `.git/config` can hand itself code
/// execution on the host: git runs `core.fsmonitor`, hooks, and filter drivers
/// straight out of repository configuration. Vitna's own host-side git is
/// hardened against that (see `vitna_git_workspaces::host_git`), but the
/// operator's own `git status` in their own terminal is not, so leaving these
/// writable turns the sandbox into a way to escape it later.
///
/// `.vitna` is here because the action journal, the event store, and the signed
/// receipts live there. Evidence a sandboxed command can rewrite is not
/// evidence.
///
/// Only paths that exist are returned, since a backend cannot protect what is
/// not there. That is a real gap: a workspace with no `.git` yet can have one
/// created inside the sandbox. It is recorded in the plan rather than papered
/// over.
pub fn protected_subpaths(workspace_root: &Path) -> Vec<PathBuf> {
    let candidates = [
        Path::new(".git").join("config"),
        Path::new(".git").join("config.worktree"),
        Path::new(".git").join("hooks"),
        Path::new(".git").join("info"),
        Path::new(".git").join("modules"),
        PathBuf::from(".vitna"),
    ];

    candidates
        .iter()
        .map(|rel| workspace_root.join(rel))
        .filter(|p| p.exists())
        .collect()
}

/// Whether `bwrap` exists and can actually create a namespace here.
///
/// Presence is not enough. Ubuntu 24.04 ships an AppArmor profile that denies
/// unprivileged user namespaces by default, so `bwrap` installs fine and then
/// fails at `setting up uid map`. Probing runs it once and caches the answer.
#[cfg(target_os = "linux")]
fn bwrap_probe() -> &'static Result<(), String> {
    static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
    PROBE.get_or_init(|| {
        let output = std::process::Command::new("bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--unshare-pid",
                "--unshare-net",
                "--die-with-parent",
                "--",
                "/bin/true",
            ])
            .output();

        match output {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!(
                "bwrap is present but cannot create a sandbox here: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Err(format!("bwrap could not be run: {}", e)),
        }
    })
}

#[cfg(target_os = "macos")]
fn seatbelt_probe() -> &'static Result<(), String> {
    static PROBE: OnceLock<Result<(), String>> = OnceLock::new();
    PROBE.get_or_init(|| {
        if !Path::new("/usr/bin/sandbox-exec").exists() {
            return Err("/usr/bin/sandbox-exec is not present".to_string());
        }
        let output = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .output();
        match output {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!(
                "sandbox-exec is present but refused a trivial profile: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => Err(format!("sandbox-exec could not be run: {}", e)),
        }
    })
}

impl SandboxConfig {
    pub fn new<P: AsRef<Path>>(workspace_root: P, guarantee: SandboxGuarantee) -> Self {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        Self {
            workspace_root: workspace_root.clone(),
            mounts: vec![SandboxMount {
                host_path: workspace_root.clone(),
                sandbox_path: workspace_root,
                writable: guarantee != SandboxGuarantee::ReadOnly,
            }],
            allow_network: false,
            max_memory_bytes: 2 * 1024 * 1024 * 1024, // 2 GB
            max_cpu_percent: 100,
            max_processes: 64,
            sanitized_environment: vec![
                ("LANG".to_string(), "en_US.UTF-8".to_string()),
                ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
            ],
            guarantee,
        }
    }

    /// Generates Bubblewrap arguments for Linux sandbox execution.
    ///
    /// The host filesystem is bound read-only and the workspace is bound over
    /// it, which is what lets a toolchain installed anywhere on the machine
    /// still run. Reads outside the workspace are therefore permitted; writes
    /// are not. That matches how comparable agent sandboxes behave and is
    /// stated plainly in `docs/PLATFORM_MATRIX.md` rather than implied.
    pub fn generate_linux_bwrap_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "bwrap",
            "--die-with-parent",
            // Its own session, so the child cannot push characters back into
            // the operator's terminal with TIOCSTI.
            "--new-session",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup-try",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        if !self.allow_network {
            args.push("--unshare-net".to_string());
        }

        for mount in &self.mounts {
            if mount.writable {
                args.push("--bind".to_string());
            } else {
                args.push("--ro-bind".to_string());
            }
            args.push(mount.host_path.to_string_lossy().to_string());
            args.push(mount.sandbox_path.to_string_lossy().to_string());
        }

        // Pin `.git` as a mount point of its own before layering the read-only
        // pieces inside it. Without this the whole directory can simply be
        // renamed and replaced, and every protection below it goes with it:
        // renaming a directory that merely contains mount points is allowed.
        let git_dir = self.workspace_root.join(".git");
        if git_dir.is_dir() {
            let p = git_dir.to_string_lossy().to_string();
            args.push("--bind".to_string());
            args.push(p.clone());
            args.push(p);
        }

        for protected in protected_subpaths(&self.workspace_root) {
            let p = protected.to_string_lossy().to_string();
            args.push("--ro-bind".to_string());
            args.push(p.clone());
            args.push(p);
        }

        args
    }

    /// Generates macOS seatbelt sandbox profile definition.
    pub fn generate_macos_seatbelt_profile(&self) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");
        // A shell that cannot fork or exec is not a usable sandbox, and a
        // profile that denies these silently produces "cannot run" for every
        // command rather than an honest refusal.
        profile.push_str("(allow process-fork)\n");
        profile.push_str("(allow process-exec)\n");
        profile.push_str("(allow signal (target same-sandbox))\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow mach-lookup)\n");
        profile.push_str("(allow ipc-posix-sem)\n");
        // Reads are allowed host-wide for the same reason the Linux profile
        // binds `/` read-only: toolchains live outside the workspace.
        profile.push_str("(allow file-read*)\n");

        let ws = self.workspace_root.to_string_lossy();
        if self.guarantee != SandboxGuarantee::ReadOnly {
            profile.push_str(&format!("(allow file-write* (require-all (subpath \"{}\")", ws));
            for protected in protected_subpaths(&self.workspace_root) {
                profile.push_str(&format!(
                    "\n  (require-not (subpath \"{}\"))",
                    protected.to_string_lossy()
                ));
            }
            // The `.git` directory entry itself, so it cannot be renamed out of
            // the way and rebuilt without the protected pieces.
            let git_dir = self.workspace_root.join(".git");
            if git_dir.exists() {
                profile.push_str(&format!(
                    "\n  (require-not (literal \"{}\"))",
                    git_dir.to_string_lossy()
                ));
            }
            profile.push_str("))\n");
        }

        if self.allow_network {
            profile.push_str("(allow network-outbound)\n");
        } else {
            profile.push_str("(deny network*)\n");
        }

        profile
    }

    /// Generates deterministic Windows AppContainer profile identifier.
    pub fn generate_windows_appcontainer_name(&self) -> String {
        let hash = sha2::Sha256::digest(self.workspace_root.to_string_lossy().as_bytes());
        format!("VitnaSandbox_{}", hex::encode(&hash[..8]))
    }
}

/// Builds the invocation that delivers `config.guarantee` for `command`.
///
/// Returns an error rather than a weaker invocation. Every caller must treat
/// that error as "do not run this", which is the whole point: the previous
/// behaviour was to run the command anyway and label it sandboxed.
pub fn plan(
    config: &SandboxConfig,
    command: &str,
    cwd: &Path,
) -> Result<SandboxPlan, PlanError> {
    match config.guarantee {
        SandboxGuarantee::FullAccess => {
            return Err(PlanError::NotImplemented {
                reason: "full_access is host execution with no sandbox and is never planned here"
                    .to_string(),
            })
        }
        SandboxGuarantee::Strong => {
            return Err(PlanError::NotImplemented {
                reason: "strong isolation means an ephemeral container or hardware VM \
                         (docs/PLATFORM_MATRIX.md); no such backend is implemented"
                    .to_string(),
            })
        }
        SandboxGuarantee::ReadOnly | SandboxGuarantee::Guarded => {}
    }

    plan_guarded(config, command, cwd)
}

#[cfg(target_os = "linux")]
fn plan_guarded(
    config: &SandboxConfig,
    command: &str,
    cwd: &Path,
) -> Result<SandboxPlan, PlanError> {
    if let Err(reason) = bwrap_probe() {
        return Err(PlanError::Unavailable {
            reason: reason.clone(),
        });
    }

    let mut args: Vec<String> = config.generate_linux_bwrap_args().split_off(1);
    args.push("--chdir".to_string());
    args.push(cwd.to_string_lossy().to_string());
    args.push("--".to_string());
    args.push("/bin/sh".to_string());
    args.push("-c".to_string());
    args.push(command.to_string());

    let mut gaps = Vec::new();
    if !config.workspace_root.join(".git").exists() {
        gaps.push(
            "workspace has no .git directory, so one created inside the sandbox is not \
             protected"
                .to_string(),
        );
    }

    Ok(SandboxPlan {
        backend: SandboxBackend::Bubblewrap,
        enforcement: if gaps.is_empty() {
            Enforcement::FullyEnforced
        } else {
            Enforcement::Fallback
        },
        gaps,
        program: "bwrap".to_string(),
        args,
        env: config.sanitized_environment.clone(),
        cwd: cwd.to_path_buf(),
    })
}

#[cfg(target_os = "macos")]
fn plan_guarded(
    config: &SandboxConfig,
    command: &str,
    cwd: &Path,
) -> Result<SandboxPlan, PlanError> {
    if let Err(reason) = seatbelt_probe() {
        return Err(PlanError::Unavailable {
            reason: reason.clone(),
        });
    }

    let profile = config.generate_macos_seatbelt_profile();
    let args = vec![
        "-p".to_string(),
        profile,
        "/bin/sh".to_string(),
        "-c".to_string(),
        command.to_string(),
    ];

    // Seatbelt confines the filesystem and the network, but it has no PID
    // namespace: a process that calls setsid escapes the group the runner
    // terminates. That is a real gap and it is named rather than assumed away.
    let mut gaps = vec![
        "no PID namespace: process-tree teardown is by process group, which a child \
         that calls setsid can leave"
            .to_string(),
    ];
    if !config.workspace_root.join(".git").exists() {
        gaps.push(
            "workspace has no .git directory, so one created inside the sandbox is not \
             protected"
                .to_string(),
        );
    }

    Ok(SandboxPlan {
        backend: SandboxBackend::Seatbelt,
        enforcement: Enforcement::Fallback,
        gaps,
        program: "/usr/bin/sandbox-exec".to_string(),
        args,
        env: config.sanitized_environment.clone(),
        cwd: cwd.to_path_buf(),
    })
}

#[cfg(windows)]
fn plan_guarded(
    _config: &SandboxConfig,
    _command: &str,
    _cwd: &Path,
) -> Result<SandboxPlan, PlanError> {
    // `generate_windows_appcontainer_name` produces a deterministic name and
    // nothing has ever created a container to go with it. An AppContainer needs
    // CreateAppContainerProfile, a capability SID, ACLs granting that SID the
    // workspace, and CreateProcess with
    // PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES. None of that exists here, so
    // the honest answer is that Windows has no sandbox yet.
    Err(PlanError::Unavailable {
        reason: "no AppContainer launcher is implemented on Windows; \
                 the existing profile name is an identifier, not a container"
            .to_string(),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn plan_guarded(
    _config: &SandboxConfig,
    _command: &str,
    _cwd: &Path,
) -> Result<SandboxPlan, PlanError> {
    Err(PlanError::Unavailable {
        reason: "no sandbox backend exists for this platform".to_string(),
    })
}
