//! Cross-platform PTY sessions built on `portable-pty` (openpty on Unix,
//! ConPTY on Windows).
//!
//! Every interactive shell spawned by the daemon goes through this module so
//! it inherits a minimal stripped environment (no ambient API keys), stays
//! bound to the session's workspace directory, sanitizes dangerous terminal
//! escape sequences (OSC 52 clipboard hijack, title injection, BiDi overrides),
//! and cleans up its child process on drop.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

/// Default width when a client leaves `terminal_columns` at 0.
pub const DEFAULT_PTY_COLUMNS: u16 = 80;
/// Default height when a client leaves `terminal_rows` at 0.
pub const DEFAULT_PTY_ROWS: u16 = 24;
/// Upper bound on either terminal dimension to prevent allocation abuse.
pub const MAX_PTY_DIMENSION: u32 = 1000;

const OUTPUT_CHANNEL_CAPACITY: usize = 256;
const READ_CHUNK_BYTES: usize = 4096;

/// Validates and normalizes `AttachTerminal` dimensions.
///
/// Zero in protobuf3 means omitted, so 0 resolves to the standard 80x24 size.
/// Values above `MAX_PTY_DIMENSION` are rejected.
pub fn normalize_dimensions(columns: u32, rows: u32) -> Result<(u16, u16), String> {
    if columns > MAX_PTY_DIMENSION || rows > MAX_PTY_DIMENSION {
        return Err(format!(
            "terminal dimensions {columns}x{rows} exceed the maximum {MAX_PTY_DIMENSION}x{MAX_PTY_DIMENSION}"
        ));
    }
    let cols = if columns == 0 {
        DEFAULT_PTY_COLUMNS
    } else {
        columns as u16
    };
    let rows = if rows == 0 {
        DEFAULT_PTY_ROWS
    } else {
        rows as u16
    };
    Ok((cols, rows))
}

/// Configuration for spawning a workspace-bound PTY session.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    pub working_dir: PathBuf,
    pub columns: u16,
    pub rows: u16,
    /// Optional explicit command + args (primarily for deterministic tests).
    /// When `None`, starts the platform shell (`/bin/sh` on Unix, `cmd.exe` on Windows).
    pub command: Option<Vec<String>>,
}

impl PtyConfig {
    pub fn shell<P: AsRef<Path>>(working_dir: P, columns: u16, rows: u16) -> Self {
        Self {
            working_dir: working_dir.as_ref().to_path_buf(),
            columns,
            rows,
            command: None,
        }
    }
}

/// Stream items broadcast to attached terminal clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Closed,
}

/// Live PTY session owned by the daemon and shared across attached clients.
pub struct PtySession {
    terminal_id: String,
    cwd: PathBuf,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    size: Mutex<(u16, u16)>,
    output_tx: broadcast::Sender<PtyEvent>,
    closed: Arc<AtomicBool>,
}

impl PtySession {
    /// Spawns a new PTY process bound to `config.working_dir` with a stripped environment.
    pub fn spawn(terminal_id: impl Into<String>, config: PtyConfig) -> Result<Arc<Self>, String> {
        if !config.working_dir.exists() || !config.working_dir.is_dir() {
            return Err(format!(
                "PTY working directory does not exist or is not a directory: {}",
                config.working_dir.display()
            ));
        }

        let (cols, rows) = normalize_dimensions(config.columns as u32, config.rows as u32)?;
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("failed to open PTY: {e}"))?;

        let mut cmd = match &config.command {
            Some(parts) if !parts.is_empty() => {
                let mut builder = CommandBuilder::new(&parts[0]);
                for arg in &parts[1..] {
                    builder.arg(arg);
                }
                builder
            }
            _ => {
                if cfg!(windows) {
                    CommandBuilder::new("cmd.exe")
                } else {
                    CommandBuilder::new("/bin/sh")
                }
            }
        };

        cmd.cwd(&config.working_dir);
        cmd.env_clear();
        for (k, v) in crate::base_environment(&[("TERM".to_string(), "xterm-256color".to_string())])
        {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("failed to spawn PTY child process: {e}"))?;
        // Drop the slave handle in the parent so EOF propagates when the child exits.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("failed to clone PTY reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("failed to acquire PTY writer: {e}"))?;

        let (output_tx, _) = broadcast::channel(OUTPUT_CHANNEL_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let terminal_id = terminal_id.into();

        let tx_reader = output_tx.clone();
        let closed_reader = Arc::clone(&closed);
        let thread_name = format!("vitna-pty-{terminal_id}");
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let mut buf = [0u8; READ_CHUNK_BYTES];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let sanitized = sanitize_pty_output(&buf[..n]);
                            if !sanitized.is_empty() {
                                let _ = tx_reader.send(PtyEvent::Output(sanitized));
                            }
                        }
                        Err(_) => break,
                    }
                }
                closed_reader.store(true, Ordering::SeqCst);
                let _ = tx_reader.send(PtyEvent::Closed);
            })
            .map_err(|e| format!("failed to spawn PTY reader thread: {e}"))?;

        Ok(Arc::new(Self {
            terminal_id,
            cwd: config.working_dir,
            master: Mutex::new(pair.master),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(child),
            size: Mutex::new((cols, rows)),
            output_tx,
            closed,
        }))
    }

    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn dimensions(&self) -> (u16, u16) {
        self.size
            .lock()
            .map(|s| *s)
            .unwrap_or((DEFAULT_PTY_COLUMNS, DEFAULT_PTY_ROWS))
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Subscribes to sanitized PTY output chunks and close notifications.
    pub fn subscribe_output(&self) -> broadcast::Receiver<PtyEvent> {
        self.output_tx.subscribe()
    }

    /// Resizes the active PTY window.
    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), String> {
        let (cols, rows) = normalize_dimensions(columns as u32, rows as u32)?;
        let master = self.master.lock().map_err(|e| e.to_string())?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("failed to resize PTY: {e}"))?;
        if let Ok(mut size) = self.size.lock() {
            *size = (cols, rows);
        }
        Ok(())
    }

    /// Writes interactive input bytes to the PTY child's stdin.
    pub fn write_input(&self, data: &[u8]) -> Result<(), String> {
        if self.is_closed() {
            return Err("PTY session is closed".to_string());
        }
        let mut guard = self.writer.lock().map_err(|e| e.to_string())?;
        let writer = guard
            .as_mut()
            .ok_or_else(|| "PTY writer is closed".to_string())?;
        writer
            .write_all(data)
            .map_err(|e| format!("failed to write to PTY: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("failed to flush PTY: {e}"))
    }

    /// Terminates the PTY child process and signals subscribers.
    pub fn close(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            writer.take();
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let was_closed = self.closed.swap(true, Ordering::SeqCst);
        if !was_closed {
            let _ = self.output_tx.send(PtyEvent::Closed);
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.close();
    }
}

/// Strips dangerous terminal control sequences from raw PTY output (THR-07):
///
/// - OSC (`ESC ] ... BEL` or `ESC ] ... ST`): blocks OSC 52 clipboard hijack,
///   OSC 0/2 title spoofing, and OSC 8 hyperlinks.
/// - DCS (`ESC P ... ST`), SOS (`ESC X ... ST`), PM (`ESC ^ ... ST`),
///   APC (`ESC _ ... ST`).
/// - Unicode BiDi directional override/isolate controls (UTF-8 encoded).
///
/// Preserves printable text, standard whitespace (`\r`, `\n`, `\t`, `\x08`),
/// and normal CSI SGR/cursor sequences (`ESC [ ...`).
pub fn sanitize_pty_output(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        let b = input[i];

        // Escape sequences starting with ESC (0x1B)
        if b == 0x1B && i + 1 < input.len() {
            match input[i + 1] {
                // OSC: ESC ] ... (BEL or ESC \)
                b']' => {
                    i += 2;
                    while i < input.len() {
                        if input[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if input[i] == 0x1B && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                // DCS (P), SOS (X), PM (^), APC (_) terminated by ST (ESC \)
                b'P' | b'X' | b'^' | b'_' => {
                    i += 2;
                    while i < input.len() {
                        if input[i] == 0x1B && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                _ => {}
            }
        }

        // UTF-8 encoded Unicode BiDi override / isolate / mark controls:
        // U+061C (D8 9C)
        if b == 0xD8 && i + 1 < input.len() && input[i + 1] == 0x9C {
            i += 2;
            continue;
        }
        // U+200E..=U+200F (E2 80 8E..=8F), U+202A..=U+202E (E2 80 AA..=AE),
        // U+2066..=U+2069 (E2 81 A6..=A9)
        if b == 0xE2 && i + 2 < input.len() {
            let b1 = input[i + 1];
            let b2 = input[i + 2];
            if (b1 == 0x80 && (matches!(b2, 0x8E..=0x8F) || matches!(b2, 0xAA..=0xAE)))
                || (b1 == 0x81 && matches!(b2, 0xA6..=0xA9))
            {
                i += 3;
                continue;
            }
        }

        out.push(b);
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "vitna_pty_{label}_{}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn normalize_dimensions_defaults_zero_and_rejects_oversized() {
        assert_eq!(normalize_dimensions(0, 0).expect("defaults"), (80, 24));
        assert_eq!(normalize_dimensions(120, 40).expect("explicit"), (120, 40));
        assert!(normalize_dimensions(MAX_PTY_DIMENSION + 1, 24).is_err());
        assert!(normalize_dimensions(80, MAX_PTY_DIMENSION + 1).is_err());
    }

    #[test]
    fn sanitize_pty_output_strips_osc52_and_bidi_while_preserving_sgr() {
        let payload = concat!(
            "\x1b[32mgreen\x1b[0m ",
            "\x1b]52;c;cHduZWQ=\x07",
            "\x1b]0;spoofed title\x1b\\",
            "\x1bPignored-dcs\x1b\\",
            "safe\u{202e}text\u{2066}ok\n"
        );
        let cleaned = sanitize_pty_output(payload.as_bytes());
        let text = String::from_utf8(cleaned).expect("valid utf8");
        assert!(text.contains("\x1b[32mgreen\x1b[0m"));
        assert!(!text.contains("cHduZWQ="));
        assert!(!text.contains("spoofed title"));
        assert!(!text.contains("ignored-dcs"));
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains('\u{2066}'));
        assert!(text.contains("safetextok\n"));
    }

    #[tokio::test]
    async fn pty_spawns_runs_command_resizes_and_closes_cleanly() {
        let dir = unique_temp_dir("roundtrip");
        let pty = PtySession::spawn("term-test-1", PtyConfig::shell(&dir, 80, 24))
            .expect("spawn PTY session");
        assert_eq!(pty.dimensions(), (80, 24));

        pty.resize(120, 40).expect("resize PTY");
        assert_eq!(pty.dimensions(), (120, 40));

        let mut rx = pty.subscribe_output();
        #[cfg(windows)]
        let cmd_bytes = b"echo vitna_pty_marker\r\nexit\r\n";
        #[cfg(not(windows))]
        let cmd_bytes = b"echo vitna_pty_marker\nexit\n";
        pty.write_input(cmd_bytes).expect("write command to PTY");

        let mut captured = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(PtyEvent::Output(chunk))) => {
                    captured.push_str(&String::from_utf8_lossy(&chunk));
                    if captured.contains("vitna_pty_marker") {
                        break;
                    }
                }
                Ok(Ok(PtyEvent::Closed)) => break,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }

        assert!(
            captured.contains("vitna_pty_marker"),
            "expected PTY output to contain marker, got: {captured:?}"
        );

        pty.close();
        assert!(pty.is_closed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn pty_strips_ambient_environment_and_sets_term() {
        let dir = unique_temp_dir("env");
        let pty = PtySession::spawn("term-test-env", PtyConfig::shell(&dir, 80, 24))
            .expect("spawn PTY session");

        let mut rx = pty.subscribe_output();
        #[cfg(windows)]
        let cmd_bytes = b"echo TERM=%TERM%\r\nexit\r\n";
        #[cfg(not(windows))]
        let cmd_bytes = b"echo TERM=$TERM\nexit\n";
        pty.write_input(cmd_bytes).expect("write env check");

        let mut captured = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Ok(PtyEvent::Output(chunk))) => {
                    captured.push_str(&String::from_utf8_lossy(&chunk));
                    if captured.contains("TERM=xterm-256color") {
                        break;
                    }
                }
                Ok(Ok(PtyEvent::Closed)) => break,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }

        assert!(
            captured.contains("TERM=xterm-256color"),
            "expected TERM=xterm-256color in PTY child env, got: {captured:?}"
        );

        pty.close();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
