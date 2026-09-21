//! Where the daemon listens, declared once.
//!
//! ADR-0005 puts `vitna-coded` on an owner-only Unix domain socket or a Windows
//! named pipe and never on loopback TCP, so that no web page can reach it. That
//! decision is the reason the desktop client is a native window rather than a
//! browser tab: the transport is one a page structurally cannot open.
//!
//! Both sides read this module. The daemon binds [`preferred`]; a client tries
//! [`candidates`] in order. They were two lists in two crates once, which is a
//! drift waiting to happen: a daemon that binds somewhere the window does not
//! look is indistinguishable, from the window, from a daemon that is not
//! running at all.

use std::path::PathBuf;

/// The endpoint this user's daemon publishes.
///
/// Returns the reason on failure rather than a fallback, because a guessed
/// path is how a client ends up reporting a healthy link to nothing.
pub fn preferred() -> Result<String, String> {
    if cfg!(windows) {
        // A pipe name is per user, so two accounts on one machine do not
        // collide. USERNAME is set by the session; without it there is no
        // per-user name to form and saying so beats sharing one pipe.
        let user = std::env::var("USERNAME").map_err(|_| {
            "USERNAME is not set, so no per-user pipe name can be formed".to_string()
        })?;
        let safe: String = user
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        if safe.is_empty() {
            return Err("USERNAME holds no usable characters for a pipe name".to_string());
        }
        Ok(format!(r"\\.\pipe\vitna-{safe}"))
    } else {
        let dir = socket_dir()?;
        Ok(dir.join("vitna.sock").to_string_lossy().to_string())
    }
}

/// The directory holding the Unix socket. The daemon creates it 0700.
#[cfg(unix)]
pub fn socket_dir() -> Result<PathBuf, String> {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime.is_empty() {
            return Ok(PathBuf::from(runtime).join("vitna"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Ok(PathBuf::from(home).join(".vitna"));
        }
    }
    Err("neither XDG_RUNTIME_DIR nor HOME is set, so no socket path is defined".to_string())
}

#[cfg(not(unix))]
pub fn socket_dir() -> Result<PathBuf, String> {
    Err("this platform has no Unix socket directory".to_string())
}

/// Every endpoint a client should try, in order.
///
/// On Unix these are exact paths. On Windows the pipe directory is also
/// enumerated for any `vitna-` name, which covers a daemon started under a
/// differently spelled account; that is a discovery step rather than a guess,
/// since a pipe either exists under that name or it does not.
pub fn candidates() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Ok(p) = preferred() {
        out.push(p);
    }

    if cfg!(windows) {
        if let Ok(entries) = std::fs::read_dir(r"\\.\pipe\") {
            let mut found: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("vitna-") || n == "vitna")
                .map(|n| format!(r"\\.\pipe\{n}"))
                .collect();
            found.sort();
            for f in found {
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    } else {
        // The other declared path, when the preferred one came from the first.
        if let Ok(home) = std::env::var("HOME") {
            let alt = PathBuf::from(home)
                .join(".vitna")
                .join("vitna.sock")
                .to_string_lossy()
                .to_string();
            if !out.contains(&alt) {
                out.push(alt);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_declared_endpoint_is_tcp() {
        // The ADR's whole point. A loopback port would be reachable from any
        // page in any browser on this machine.
        for endpoint in candidates() {
            assert!(!endpoint.contains("127.0.0.1"), "got {endpoint}");
            assert!(!endpoint.contains("localhost"), "got {endpoint}");
            assert!(!endpoint.contains("://"), "got {endpoint}");
        }
    }

    #[test]
    fn the_preferred_endpoint_is_one_a_client_will_try() {
        // If these two disagree the daemon binds somewhere the window never
        // looks, and the window reports an absent daemon forever.
        if let Ok(p) = preferred() {
            assert!(
                candidates().contains(&p),
                "preferred {p} is missing from the probe list {:?}",
                candidates()
            );
        }
    }
}
