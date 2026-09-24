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
//!
//! Where it listens and keeps its journal, and how it starts, live in
//! `vitna_daemon::launch`, which `vitna serve` starts through as well, so the
//! two doors to this daemon cannot disagree.

use clap::Parser;
use vitna_daemon::launch::{self, Launch};

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

#[tokio::main]
async fn main() {
    launch::logging();

    if let Err(e) = run().await {
        // One line naming what failed. A daemon that dies silently is
        // indistinguishable, from a client, from one that was never started.
        eprintln!("vitna-coded: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args = Args::parse();
    let launch = Launch::resolve(args.endpoint, args.store)?;

    let announce = launch.endpoint.clone();
    launch::run(&launch, move || {
        tracing::info!(endpoint = %announce, "listening");
        // On stdout as well as in the log, because it is the line a script or
        // a test waits for before connecting.
        println!("vitna-coded listening on {announce}");
    })
    .await
}
