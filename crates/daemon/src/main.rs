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
//! It signs with this account's device key, kept owner-only in `~/.vitna`
//! (`vitna_daemon::key`), so a receipt signed by one run of this binary checks
//! against the key of the next. Until that module, it made a fresh key on every
//! start, and no receipt it signed could be checked a minute later.
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

    /// A vitna-runtime on this machine, such as http://127.0.0.1:8787. Turns
    /// are refused until it and --lane are given.
    #[arg(long)]
    runtime_url: Option<String>,

    /// The lane every turn asks the runtime for, such as vitna/code.
    #[arg(long)]
    lane: Option<String>,

    /// Model provider (`anthropic`, `openai`, or local `vitna-anchor`).
    /// Anthropic/OpenAI keys are read strictly from the OS keychain (`vitna auth set`).
    #[arg(long)]
    provider: Option<String>,

    /// Model SKU for --provider (for example `claude-sonnet-4-20250514`, `gpt-4o`, or `qwen3-coder`).
    #[arg(long)]
    model: Option<String>,

    /// Optional custom HTTP/HTTPS API endpoint override for --provider.
    #[arg(long)]
    base_url: Option<String>,

    /// Local checkpoint file or directory hashed for `vitna-anchor` receipt attestation.
    #[arg(long)]
    checkpoint: Option<std::path::PathBuf>,

    /// Attach a Streamable HTTP MCP server (`<name>=<url>`). Repeatable.
    #[arg(long)]
    mcp_http: Vec<String>,

    /// Attach a local stdio MCP server (`<name>=<program> [args...]`). Repeatable.
    #[arg(long)]
    mcp_stdio: Vec<String>,

    /// Allowlist a destination host or `*.domain.tld` for brokered `net:http_fetch`. Repeatable.
    #[arg(long = "allow-egress")]
    allow_egress: Vec<String>,
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
    let launch = Launch::resolve(args.endpoint, args.store)?
        .with_runtime(args.runtime_url, args.lane)?
        .with_direct_provider(args.provider, args.model, args.base_url)?
        .with_checkpoint(args.checkpoint)?
        .with_mcp_servers(args.mcp_http, args.mcp_stdio)?
        .with_egress_allowlist(args.allow_egress)?;

    let announce = launch.endpoint.clone();
    launch::run(&launch, move || {
        tracing::info!(endpoint = %announce, "listening");
        // On stdout as well as in the log, because it is the line a script or
        // a test waits for before connecting.
        println!("vitna-coded listening on {announce}");
    })
    .await
}
