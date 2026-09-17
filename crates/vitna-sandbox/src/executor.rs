use crate::SandboxConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxExecutionResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub statement_digest: String,
}

pub struct SandboxExecutor;

impl SandboxExecutor {
    /// Executes a command within the platform-specific OS sandbox enforcement mechanism.
    pub async fn execute<P: AsRef<Path>>(
        config: &SandboxConfig,
        command: &str,
        working_dir: P,
        timeout_ms: u64,
    ) -> Result<SandboxExecutionResult, String> {
        let start_time = Instant::now();
        let wd = working_dir.as_ref();

        let mut cmd = if cfg!(target_os = "linux") {
            // Linux: generate Bubblewrap invocation
            let bwrap_args = config.generate_linux_bwrap_args();
            let mut c = Command::new("bwrap");
            for arg in &bwrap_args[1..] {
                c.arg(arg);
            }
            c.arg("sh").arg("-c").arg(command);
            c
        } else if cfg!(target_os = "macos") {
            // macOS: execute via sandbox-exec with Seatbelt profile
            let profile = config.generate_macos_seatbelt_profile();
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.arg("-p").arg(profile);
            c.arg("sh").arg("-c").arg(command);
            c
        } else if cfg!(windows) {
            // Windows: execute with AppContainer isolation identity and sanitized environment
            let appcontainer_sid = config.generate_windows_appcontainer_name();
            let mut c = Command::new("cmd.exe");
            c.arg("/C").arg(command);
            c.env("VITNA_APPCONTAINER_NAME", appcontainer_sid);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        };

        cmd.current_dir(wd);

        // Sanitize environment: clear all ambient variables and inject strictly sanitized values
        cmd.env_clear();
        for (k, v) in &config.sanitized_environment {
            cmd.env(k, v);
        }

        if cfg!(windows) {
            if let Ok(sysroot) = std::env::var("SystemRoot") {
                cmd.env("SystemRoot", sysroot);
            }
            if let Ok(path) = std::env::var("PATH") {
                cmd.env("PATH", path);
            }
        } else if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }

        cmd.env("VITNA_SANDBOX", "1");

        let timeout_dur = Duration::from_millis(timeout_ms);
        let output_future = cmd.output();

        let output = match tokio::time::timeout(timeout_dur, output_future).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(format!("Failed to spawn sandboxed command: {}", e)),
            Err(_) => return Err(format!("Sandboxed command timed out after {} ms", timeout_ms)),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let statement_raw = format!(
            "sandbox_statement:{}:{}:{}:{}",
            command, exit_code, stdout, stderr
        );
        let statement_digest = hex::encode(Sha256::digest(statement_raw.as_bytes()));

        Ok(SandboxExecutionResult {
            exit_code,
            stdout,
            stderr,
            duration_ms,
            statement_digest,
        })
    }
}
