//! Interactive PTY attachment for daemon sessions (`AttachTerminal`).
//!
//! Each session owns at most one active `PtySession` bound to its
//! `workspace_root` with a stripped environment (via `vitna_runner::pty`).
//! Re-attaching with new dimensions resizes the existing PTY in place rather
//! than spawning duplicate shells.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vitna_protocol::messages::AttachTerminal;
use vitna_runner::pty::{normalize_dimensions, PtyConfig, PtySession};

use crate::DaemonServer;

static TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Errors returned when attaching or resizing a session terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalError {
    UnknownSession(String),
    InvalidDimensions(String),
    SpawnFailed(String),
}

impl std::fmt::Display for TerminalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSession(m) | Self::InvalidDimensions(m) | Self::SpawnFailed(m) => {
                f.write_str(m)
            }
        }
    }
}

/// Result of a successful `DaemonServer::attach_terminal` call.
#[derive(Clone)]
pub struct TerminalAttachment {
    pub terminal_id: String,
    pub session_id: String,
    pub columns: u16,
    pub rows: u16,
    pub cwd: PathBuf,
    /// `true` when a new PTY process was spawned; `false` when an existing
    /// session PTY was resized in place.
    pub created: bool,
    pub pty: Arc<PtySession>,
}

impl DaemonServer {
    /// Attaches (or resizes) a workspace-bound PTY terminal for `req.session_id`.
    pub fn attach_terminal(&self, req: &AttachTerminal) -> Result<TerminalAttachment, TerminalError> {
        let session = self.get_session(&req.session_id).ok_or_else(|| {
            TerminalError::UnknownSession(format!("no session {}", req.session_id))
        })?;

        let (cols, rows) = normalize_dimensions(req.terminal_columns, req.terminal_rows)
            .map_err(TerminalError::InvalidDimensions)?;

        // Reuse an active PTY for this session if one is still alive.
        let existing = {
            let mut terminals = self
                .terminals
                .lock()
                .map_err(|e| TerminalError::SpawnFailed(e.to_string()))?;
            match terminals.get(&session.session_id) {
                Some(pty) if !pty.is_closed() => Some(Arc::clone(pty)),
                Some(_) => {
                    terminals.remove(&session.session_id);
                    None
                }
                None => None,
            }
        };

        if let Some(pty) = existing {
            pty.resize(cols, rows)
                .map_err(TerminalError::SpawnFailed)?;
            return Ok(TerminalAttachment {
                terminal_id: pty.terminal_id().to_string(),
                session_id: session.session_id,
                columns: cols,
                rows,
                cwd: pty.cwd().to_path_buf(),
                created: false,
                pty,
            });
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let seq = TERMINAL_COUNTER.fetch_add(1, Ordering::SeqCst);
        let terminal_id = format!("term-{now_ms}-{seq}");

        let pty = PtySession::spawn(
            terminal_id.clone(),
            PtyConfig::shell(&session.workspace_root, cols, rows),
        )
        .map_err(TerminalError::SpawnFailed)?;

        {
            let mut terminals = self
                .terminals
                .lock()
                .map_err(|e| TerminalError::SpawnFailed(e.to_string()))?;
            terminals.insert(session.session_id.clone(), Arc::clone(&pty));
        }

        Ok(TerminalAttachment {
            terminal_id,
            session_id: session.session_id,
            columns: cols,
            rows,
            cwd: session.workspace_root,
            created: true,
            pty,
        })
    }

    /// Writes interactive bytes into the active PTY attached to `session_id`.
    pub fn write_terminal_input(&self, session_id: &str, data: &[u8]) -> Result<(), String> {
        let pty = {
            let terminals = self.terminals.lock().map_err(|e| e.to_string())?;
            terminals
                .get(session_id)
                .cloned()
                .ok_or_else(|| format!("no active terminal for session {session_id}"))?
        };
        pty.write_input(data)
    }

    /// Closes and removes the active PTY attached to `session_id`, if any.
    pub fn detach_terminal(&self, session_id: &str) -> bool {
        let removed = self
            .terminals
            .lock()
            .ok()
            .and_then(|mut map| map.remove(session_id));
        if let Some(pty) = removed {
            pty.close();
            true
        } else {
            false
        }
    }
}
