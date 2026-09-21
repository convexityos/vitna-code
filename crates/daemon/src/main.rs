//! `vitna-coded`, the local daemon.
//!
//! Until this file served it, the binary printed one line and exited, so the
//! listener in `ipc` was reachable only from tests. That was the state of the
//! pull request that added the listener, whose title said "vitna-coded
//! listens"; a desktop client probing for the daemon would have found nothing
//! however long it waited. `tests/binary_serves.rs` now starts THIS binary and
//! completes a handshake with it, so the claim is checked against the program
//! that ships rather than against the library it is built from.
//!
//! Startup follows the native-desktop-app branch, minus its provider check:
//! that read ANTHROPIC_API_KEY and OPENAI_API_KEY from the environment, which
//! CONTRIBUTING's fourth rule forbids.
//!
//! Known limitation: `DaemonServer::open_default` generates a fresh signing
//! key on every start, so a receipt signed by one run of this binary cannot be
//! checked against the key of the next. Keeping one key, owner-only, is a
//! separate change.

use std::sync::Arc;

use clap::Parser;
use vitna_daemon::{ipc, DaemonServer};

#[derive(Parser, Debug)]
#[command(name = "vitna-coded", about = "The Vitna Code local daemon")]
struct Args {
    /// Where to listen. Defaults to the endpoint ADR-0005 declares for this
    /// user, which is the one a client probes.
    #[arg(long)]
    endpoint: Option<String>,

    /// The event store. Defaults to `.vitna/daemon.db` under the home
    /// directory, so runs from any workspace share one journal.
    #[arg(long)]
    store: Option<std::path::PathBuf>,
}

fn default_store() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "neither HOME nor USERPROFILE is set, so --store must be given".to_string())?;
    Ok(std::path::PathBuf::from(home)
        .join(".vitna")
        .join("daemon.db"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Err(e) = run().await {
        // One line naming what failed. A daemon that dies silently is
        // indistinguishable, from a client, from one that was never started.
        eprintln!("vitna-coded: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = Args::parse();

    let endpoint = match args.endpoint {
        Some(e) => e,
        None => vitna_protocol::endpoint::preferred()?,
    };
    let store_path = match args.store {
        Some(p) => p,
        None => default_store()?,
    };

    let server = Arc::new(DaemonServer::open_default(&store_path)?);
    tracing::info!(store = %store_path.display(), "vitna-coded starting");

    let announce = endpoint.clone();
    ipc::serve(server, &endpoint, move || {
        tracing::info!(endpoint = %announce, "listening");
        // On stdout as well as in the log, because it is the line a script or
        // a test waits for before connecting.
        println!("vitna-coded listening on {announce}");
    })
    .await
    .map_err(|e| format!("listening on {endpoint} failed: {e}"))
}
