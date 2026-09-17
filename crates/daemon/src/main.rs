//! `vitna-coded`, the local daemon.
//!
//! This binary printed one line and exited until the IPC module existed. The
//! `DaemonServer` it now serves had been sitting in `server.rs` beside it the
//! whole time, tested and unreachable, because nothing ever bound a socket.

use std::sync::Arc;

use clap::Parser;
use vitna_daemon::{ipc, DaemonServer};

#[derive(Parser, Debug)]
#[command(name = "vitna-coded", about = "The Vitna Code local daemon")]
struct Args {
    /// Where to listen. Defaults to the endpoint ADR-0005 declares for this
    /// user, which is the one the desktop window probes.
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
        // One line, naming the thing that failed. A daemon that dies silently
        // is indistinguishable from one that was never started.
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

    let ready = vitna_daemon::providers_ready();
    tracing::info!(
        store = %store_path.display(),
        providers_ready = ready.len(),
        "vitna-coded starting"
    );
    if ready.is_empty() {
        // Said once at startup rather than discovered per turn, since a turn
        // that fails on a missing key looks like a model problem.
        tracing::warn!(
            "no provider credential is set; turns will be refused until ANTHROPIC_API_KEY or OPENAI_API_KEY is present"
        );
    }

    ipc::serve(server, &endpoint, |e| {
        tracing::info!(endpoint = %e, "listening");
        println!("vitna-coded listening on {e}");
    })
    .await
    .map_err(|e| format!("listening on {endpoint} failed: {e}"))
}
