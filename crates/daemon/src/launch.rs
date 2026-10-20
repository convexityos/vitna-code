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

use vitna_providers::{RuntimeLane, RuntimeLaneProvider};

use crate::{ipc, DaemonServer, ModelRoute};

/// Where the daemon listens, where it keeps its journal, and which model a
/// turn reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub endpoint: String,
    pub store: PathBuf,
    /// The vitna-runtime lane turns are served through. `None` starts a
    /// daemon that refuses `SubmitTurn` and says why.
    pub route: Option<RuntimeLane>,
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
        Ok(Launch {
            endpoint,
            store,
            route: None,
        })
    }

    /// Serves turns through a vitna-runtime lane: `--runtime-url` and
    /// `--lane`, both or neither. One without the other is refused rather than
    /// defaulted, because a default lane is a model nobody chose, and a
    /// default address is a runtime nobody started.
    pub fn with_runtime(
        mut self,
        runtime_url: Option<String>,
        lane: Option<String>,
    ) -> Result<Launch, String> {
        self.route = match (runtime_url, lane) {
            (None, None) => None,
            (Some(url), Some(lane)) => Some(RuntimeLane::new(&url, &lane)?),
            (Some(_), None) => {
                return Err("--runtime-url needs --lane, the lane to ask for".to_string())
            }
            (None, Some(_)) => {
                return Err("--lane needs --runtime-url, where the runtime listens".to_string())
            }
        };
        Ok(self)
    }
}

/// `.vitna/daemon.db` under the home directory.
///
/// An empty `HOME` counts as unset. Joined onto an empty path the journal would
/// land beside the current folder instead, which is the defect this module
/// exists to remove.
pub fn default_store() -> Result<PathBuf, String> {
    vitna_home()
        .map(|home| home.join("daemon.db"))
        .map_err(|e| format!("{e}, so --store must be given"))
}

/// `.vitna` under the home directory: this account's Vitna folder, where the
/// daemon keeps its journal by default and the device key always.
pub fn vitna_home() -> Result<PathBuf, String> {
    let home = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .ok_or_else(|| "neither HOME nor USERPROFILE is set".to_string())?;
    Ok(PathBuf::from(home).join(".vitna"))
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
    let mut server = DaemonServer::open_default(&launch.store)?;
    match &launch.route {
        Some(lane) => {
            tracing::info!(runtime = %lane.endpoint, lane = %lane.lane, "turns go to a vitna-runtime lane");
            server = server.with_route(ModelRoute {
                provider: Arc::new(RuntimeLaneProvider::new(lane.clone())),
                model: lane.lane.clone(),
                routing_reason: format!("runtime_lane:{}", lane.lane),
            });
        }
        None => tracing::info!(
            "no model route: SubmitTurn is refused until --runtime-url and --lane are given"
        ),
    }
    let server = Arc::new(server);
    tracing::info!(store = %launch.store.display(), "vitna-coded starting");
    ipc::serve(server, &launch.endpoint, on_ready)
        .await
        .map_err(|e| format!("listening on {} failed: {e}", launch.endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both or neither. One alone is a route half described, and defaulting
    /// the other half would pick a model or a runtime nobody named.
    #[test]
    fn a_runtime_route_takes_both_flags_or_neither() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();
        assert_eq!(base().with_runtime(None, None).unwrap().route, None);
        let lane = base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/code".into()),
            )
            .unwrap()
            .route
            .expect("a route");
        assert_eq!(lane.endpoint, "http://127.0.0.1:8787/v1/chat/completions");
        assert_eq!(lane.lane, "vitna/code");
        assert!(base()
            .with_runtime(Some("http://127.0.0.1:8787".into()), None)
            .is_err());
        assert!(base()
            .with_runtime(None, Some("vitna/code".into()))
            .is_err());
        assert!(base()
            .with_runtime(
                Some("https://runtime.example.com".into()),
                Some("vitna/code".into())
            )
            .is_err());
    }

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
