//! Starting the daemon, one way, whichever door it is started through.
//!
//! `vitna-coded` and `vitna serve` are two names for one daemon. Until this
//! module each decided for itself where to listen and where to keep the
//! journal, and `vitna serve` decided wrong: it opened a store beside whatever
//! folder it was run from, printed "Vitna daemon listening on local session
//! channel", and bound nothing, so a client probing the declared endpoint found
//! no daemon however long it waited. The release scripts ship `vitna` and not
//! `vitna-coded`, which made that the only door an installed copy had.
//!
//! Both binaries now resolve their endpoint and journal here and start through
//! [`run`], so the two cannot drift apart again.

use std::path::PathBuf;
use std::sync::Arc;

use crate::{ipc, DaemonServer};

/// Where the daemon listens, and where it keeps its journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub endpoint: String,
    pub store: PathBuf,
}

impl Launch {
    /// What was asked for, or else the defaults: the endpoint ADR-0005 declares
    /// for this user, which is the one every client probes, and
    /// `.vitna/daemon.db` under the home directory, so a daemon started from
    /// any folder keeps the one journal.
    ///
    /// Refuses rather than guessing when a default cannot be formed, because a
    /// guessed endpoint is a daemon no client will find, and a guessed journal
    /// is a second history nobody knows to look in.
    pub fn resolve(endpoint: Option<String>, store: Option<PathBuf>) -> Result<Launch, String> {
        let endpoint = match endpoint {
            Some(e) => e,
            None => vitna_protocol::endpoint::preferred()?,
        };
        let store = match store {
            Some(s) => s,
            None => default_store()?,
        };
        Ok(Launch { endpoint, store })
    }
}

/// `.vitna/daemon.db` under the home directory.
///
/// An empty `HOME` counts as unset. Joined onto an empty path the journal would
/// land beside the current folder instead, which is the defect this module
/// exists to remove.
pub fn default_store() -> Result<PathBuf, String> {
    let home = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .ok_or_else(|| {
            "neither HOME nor USERPROFILE is set, so --store must be given".to_string()
        })?;
    Ok(PathBuf::from(home).join(".vitna").join("daemon.db"))
}

/// The daemon's log, on stdout, at `info` unless `RUST_LOG` says otherwise.
/// Installs the process's one global subscriber, so it is called once, by the
/// binary about to run the daemon.
pub fn logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// Opens the journal and serves the declared wire until the listener fails.
///
/// `on_ready` runs once the endpoint is bound and not before, so a line it
/// prints is true when it is printed.
pub async fn run<F: FnOnce()>(launch: &Launch, on_ready: F) -> Result<(), String> {
    let server = Arc::new(DaemonServer::open_default(&launch.store)?);
    tracing::info!(store = %launch.store.display(), "vitna-coded starting");
    ipc::serve(server, &launch.endpoint, on_ready)
        .await
        .map_err(|e| format!("listening on {} failed: {e}", launch.endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_asked_for_is_what_is_used() {
        let launch = Launch::resolve(Some("an-endpoint".into()), Some(PathBuf::from("a.db")))
            .expect("nothing to default");
        assert_eq!(launch.endpoint, "an-endpoint");
        assert_eq!(launch.store, PathBuf::from("a.db"));
    }

    /// The defaults are the endpoint a client probes and one journal under the
    /// home directory, whatever folder the daemon is started from.
    #[test]
    fn the_defaults_are_the_probed_endpoint_and_one_journal_under_home() {
        let (Ok(endpoint), Ok(store)) = (vitna_protocol::endpoint::preferred(), default_store())
        else {
            eprintln!(
                "skipped: this machine has no per-user endpoint or home directory to default to"
            );
            return;
        };
        let launch = Launch::resolve(None, None).expect("the defaults resolve");
        assert_eq!(launch.endpoint, endpoint);
        assert!(
            vitna_protocol::endpoint::candidates().contains(&launch.endpoint),
            "the daemon would bind {} where no client looks",
            launch.endpoint
        );
        assert_eq!(launch.store, store);
        assert!(
            launch
                .store
                .ends_with(std::path::Path::new(".vitna").join("daemon.db")),
            "{}",
            launch.store.display()
        );
        assert!(
            launch.store.is_absolute(),
            "a relative journal moves with the folder the daemon was started from: {}",
            launch.store.display()
        );
    }
}
