//! Platform-specific OS sandbox enforcement abstractions (Bubblewrap, AppContainer, Landlock).

pub mod executor;

pub use executor::{SandboxExecutionResult, SandboxExecutor};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxGuarantee {
    ReadOnly,
    Guarded,
    Strong,
    FullAccess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMount {
    pub host_path: PathBuf,
    pub sandbox_path: PathBuf,
    pub writable: bool,
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
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("LANG".to_string(), "en_US.UTF-8".to_string()),
            ],
            guarantee,
        }
    }

    /// Generates Bubblewrap arguments for Linux sandbox execution.
    pub fn generate_linux_bwrap_args(&self) -> Vec<String> {
        let mut args = vec![
            "bwrap".to_string(),
            "--die-with-parent".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-ipc".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--ro-bind".to_string(),
            "/usr".to_string(),
            "/usr".to_string(),
            "--ro-bind".to_string(),
            "/lib".to_string(),
            "/lib".to_string(),
        ];

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

        args
    }

    /// Generates macOS seatbelt sandbox profile definition.
    pub fn generate_macos_seatbelt_profile(&self) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");
        profile.push_str("(allow process-exec (subpath \"/usr/bin\") (subpath \"/bin\"))\n");
        profile.push_str("(allow file-read* (subpath \"/usr\") (subpath \"/System\") (subpath \"/lib\"))\n");

        let ws = self.workspace_root.to_string_lossy();
        if self.guarantee != SandboxGuarantee::ReadOnly {
            profile.push_str(&format!("(allow file-read* file-write* (subpath \"{}\"))\n", ws));
        } else {
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", ws));
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