//! The link to `vitna-coded`.
//!
//! ADR-0005 puts the daemon on an owner-only Unix domain socket or a Windows
//! named pipe and never on loopback TCP, so that no web page can reach it.
//! That decision is the reason this window is a native application rather than
//! a browser tab: the transport is one a page structurally cannot open.
//!
//! Today the probe always comes back empty, because `vitna-coded` prints one
//! line and exits without ever listening. That is reported as what it is, an
//! absent daemon with the endpoint that was tried, and never as an idle or
//! healthy link. A window that cannot reach the daemon says so.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Nothing is listening. Carries the endpoints that were tried, so the
    /// reading is checkable rather than a shrug.
    Absent { tried: Vec<String>, detail: String },
    /// A listener accepted the connection.
    Open { endpoint: String },
}

impl Link {
    pub fn is_open(&self) -> bool {
        matches!(self, Link::Open { .. })
    }
}

/// The endpoints ADR-0005 declares, in the order the ADR lists them.
///
/// On Unix these are exact paths. On Windows the pipe is named for the user's
/// SID, which this process cannot read without Win32 calls, so the pipe
/// directory is listed and any `vitna-` pipe is taken as the candidate. That is
/// a discovery step rather than a guess: a pipe either exists under that name
/// or it does not.
pub fn endpoints() -> Vec<String> {
    if cfg!(windows) {
        let mut found: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(r"\\.\pipe\") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("vitna-") || name == "vitna" {
                    found.push(format!(r"\\.\pipe\{name}"));
                }
            }
        }
        found.sort();
        found
    } else {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
            paths.push(PathBuf::from(runtime).join("vitna").join("vitna.sock"));
        }
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join(".vitna").join("vitna.sock"));
        }
        paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }
}

/// Tries each declared endpoint once and reports the first that accepts.
pub fn probe() -> Link {
    let tried = endpoints();

    if tried.is_empty() {
        return Link::Absent {
            tried,
            detail: if cfg!(windows) {
                "no pipe named vitna- is published".to_string()
            } else {
                "neither XDG_RUNTIME_DIR nor HOME is set, so no socket path is defined".to_string()
            },
        };
    }

    let mut detail = String::new();
    for endpoint in &tried {
        match connect(endpoint) {
            Ok(()) => {
                return Link::Open {
                    endpoint: endpoint.clone(),
                }
            }
            Err(err) => {
                if !detail.is_empty() {
                    detail.push_str("; ");
                }
                detail.push_str(&format!("{endpoint}: {err}"));
            }
        }
    }

    Link::Absent { tried, detail }
}

#[cfg(windows)]
fn connect(endpoint: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
fn connect(endpoint: &str) -> Result<(), String> {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(endpoint).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(any(windows, unix)))]
fn connect(_endpoint: &str) -> Result<(), String> {
    Err("this platform has no declared local IPC transport".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_daemon_is_reported_as_absent_and_never_as_open() {
        // vitna-coded does not listen, so this is the state the window is in
        // today. The point of the assertion is that the absent case carries its
        // evidence rather than collapsing to a bare false.
        let link = probe();
        match link {
            Link::Absent { tried, detail } => {
                assert!(
                    !detail.is_empty(),
                    "an absent daemon states why, so the window can print it"
                );
                for endpoint in tried {
                    assert!(
                        !endpoint.starts_with("127.0.0.1") && !endpoint.starts_with("localhost"),
                        "ADR-0005 forbids a loopback TCP endpoint, got {endpoint}"
                    );
                }
            }
            Link::Open { endpoint } => {
                // If some future daemon is genuinely listening while these tests
                // run, that is a pass too, but it still may not be TCP.
                assert!(!endpoint.starts_with("127.0.0.1"), "not a TCP endpoint");
            }
        }
    }

    #[test]
    fn no_declared_endpoint_is_tcp() {
        for endpoint in endpoints() {
            assert!(!endpoint.contains("127.0.0.1"));
            assert!(!endpoint.contains("localhost"));
        }
    }
}
