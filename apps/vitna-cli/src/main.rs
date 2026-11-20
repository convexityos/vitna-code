//! Vitna CLI
//!
//! Non-interactive command-line interface and machine runner for Vitna Code.

mod commands;

use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use vitna_daemon::DaemonServer;
use vitna_receipt_verify::verify_receipt_json;

#[derive(Parser, Debug)]
#[command(name = "vitna")]
#[command(about = "Vitna Code: The coding terminal that leaves a receipt", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Execute a task in non-interactive or batch mode
    Run {
        task: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "build")]
        mode: String,
        #[arg(long)]
        auto_approve: bool,
        #[arg(long)]
        verify_cmd: Option<String>,
        /// Run commands with no OS sandbox when none is available.
        ///
        /// Separate from --auto-approve on purpose: that one approves the work,
        /// this one approves removing the boundary around it. Without it, a
        /// command is refused rather than run unprotected.
        #[arg(long)]
        allow_unsandboxed: bool,
        /// A local vitna-runtime URL (loopback only, e.g. http://127.0.0.1:8787)
        /// for running a live model-driven agent turn. Requires --lane.
        #[arg(long)]
        runtime_url: Option<String>,
        /// The vitna-runtime lane alias (e.g. vitna/code). Requires --runtime-url.
        #[arg(long)]
        lane: Option<String>,
        /// Direct model provider (`anthropic`, `openai`, or local `vitna-anchor`).
        /// Requires --model and cannot be combined with --runtime-url/--lane.
        #[arg(long)]
        provider: Option<String>,
        /// Model SKU for a direct provider route (for example `claude-sonnet-4-5`, `gpt-5`, or `qwen3-coder`).
        #[arg(long)]
        model: Option<String>,
        /// Optional explicit http:// or https:// endpoint override for --provider/--model.
        #[arg(long)]
        base_url: Option<String>,
        /// Optional local checkpoint file or directory hashed for `vitna-anchor` receipt attestation.
        #[arg(long)]
        checkpoint: Option<PathBuf>,
        /// Attach a Streamable HTTP MCP server for this run (`NAME=URL`). Repeatable.
        #[arg(long = "mcp-http", value_name = "NAME=URL")]
        mcp_http: Vec<String>,
        /// Attach a stdio MCP server for this run (`NAME=COMMAND [ARGS...]`). Repeatable.
        #[arg(long = "mcp-stdio", value_name = "NAME=CMD")]
        mcp_stdio: Vec<String>,
        /// Allowlist a destination host or wildcard subdomain for brokered `net:http_fetch`
        /// (for example `api.github.com`, `*.docs.rs`, or `127.0.0.1`). Repeatable.
        #[arg(long = "allow-egress", value_name = "HOST")]
        allow_egress: Vec<String>,
        /// Sandbox guarantee for command execution (`read_only`, `guarded`, or `strong`).
        #[arg(long = "sandbox-guarantee")]
        sandbox_guarantee: Option<String>,
        /// Explicit OCI container image required when `--sandbox-guarantee strong` is set.
        #[arg(long = "sandbox-image")]
        sandbox_image: Option<String>,
        /// Path to a `vitna-mandate-config-v1` JSON file enforcing an `am/1` Agent Mandate chain.
        #[arg(long)]
        mandate: Option<PathBuf>,
        /// Optional SQLite store path (defaults to <workspace>/.vitna/store.db).
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Resume an existing session from the persisted store or submit a follow-up turn
    Resume {
        session_id: Option<String>,
        /// Optional SQLite store path (defaults to .vitna/store.db or ~/.vitna/daemon.db).
        #[arg(long)]
        store: Option<PathBuf>,
        /// Submit a follow-up prompt to a running daemon for this session.
        #[arg(long)]
        prompt: Option<String>,
        /// Override the daemon IPC endpoint when sending --prompt.
        #[arg(long)]
        endpoint: Option<String>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Review working-tree changes against signed receipt evidence
    Review {
        path_or_ref: Option<String>,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Validate preimages and apply an approved change set to the primary checkout
    Apply {
        change_set_id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Source workspace directory holding the verified postimage files to apply.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Validate preimages only without modifying any files on disk.
        #[arg(long, alias = "dry-run")]
        check: bool,
    },
    /// List active and previous sessions
    Sessions {
        /// Optional SQLite store path (defaults to .vitna/store.db or ~/.vitna/daemon.db).
        #[arg(long)]
        store: Option<PathBuf>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Receipt operations
    Receipt {
        #[command(subcommand)]
        sub: ReceiptCommands,
    },
    /// Verify an exported receipt file independently
    Verify {
        receipt_file: String,
        /// The public key (hex) the receipt must be signed with. Without it,
        /// the signature is checked against this account's device key and a
        /// receipt signed elsewhere is reported as such, not failed.
        #[arg(long)]
        key: Option<String>,
    },
    /// Check local environment, sandbox backend, and daemon health
    Doctor,
    /// Start the local daemon: the same daemon, endpoint and journal as vitna-coded
    Serve {
        /// Where to listen. Defaults to the endpoint every client probes.
        #[arg(long)]
        endpoint: Option<String>,
        /// The event store. Defaults to `.vitna/daemon.db` under the home
        /// directory. `--db`, the flag's name before it matched vitna-coded's,
        /// still works.
        #[arg(long, alias = "db")]
        store: Option<PathBuf>,
        /// A vitna-runtime on this machine, such as http://127.0.0.1:8787.
        /// Turns are refused until it and --lane (or --provider/--model) are given.
        #[arg(long)]
        runtime_url: Option<String>,
        /// The lane every turn asks the runtime for, such as vitna/code.
        #[arg(long)]
        lane: Option<String>,
        /// Direct model provider (`anthropic`, `openai`, or local `vitna-anchor`).
        /// Requires --model and cannot be combined with --runtime-url/--lane.
        #[arg(long)]
        provider: Option<String>,
        /// Model SKU for a direct provider route (for example `claude-sonnet-4-5`, `gpt-5`, or `qwen3-coder`).
        #[arg(long)]
        model: Option<String>,
        /// Optional explicit http:// or https:// endpoint override for --provider/--model.
        #[arg(long)]
        base_url: Option<String>,
        /// Optional local checkpoint file or directory hashed for `vitna-anchor` receipt attestation.
        #[arg(long)]
        checkpoint: Option<PathBuf>,
        /// Attach a Streamable HTTP MCP server at startup (`NAME=URL`). Repeatable.
        #[arg(long = "mcp-http", value_name = "NAME=URL")]
        mcp_http: Vec<String>,
        /// Attach a stdio MCP server at startup (`NAME=COMMAND [ARGS...]`). Repeatable.
        #[arg(long = "mcp-stdio", value_name = "NAME=CMD")]
        mcp_stdio: Vec<String>,
        /// Allowlist a destination host or wildcard subdomain for brokered `net:http_fetch`
        /// (for example `api.github.com`, `*.docs.rs`, or `127.0.0.1`). Repeatable.
        #[arg(long = "allow-egress", value_name = "HOST")]
        allow_egress: Vec<String>,
        /// Sandbox guarantee for command execution (`read_only`, `guarded`, or `strong`).
        #[arg(long = "sandbox-guarantee")]
        sandbox_guarantee: Option<String>,
        /// Explicit OCI container image required when `--sandbox-guarantee strong` is set.
        #[arg(long = "sandbox-image")]
        sandbox_image: Option<String>,
        /// Path to a `vitna-mandate-config-v1` JSON file enforcing an `am/1` Agent Mandate chain.
        #[arg(long)]
        mandate: Option<PathBuf>,
    },
    /// Manage customer-held provider credentials in the OS keychain (never scans env vars)
    Auth {
        #[command(subcommand)]
        sub: AuthCommands,
    },
    /// Inspect and verify workspace agent skills (.vitna/skills)
    Skills {
        #[command(subcommand)]
        sub: SkillsCommands,
    },
    /// Open a folder in the Vitna Code interface, served from this machine
    ///
    /// The page reads and writes the folder in place, runs commands inside the
    /// OS sandbox, and has each run's receipt signed with this account's
    /// device key. It still calls the model itself, on your own OpenRouter
    /// key; nothing here holds a provider key or calls a model.
    App {
        /// The folder to open. Defaults to the current folder.
        folder: Option<PathBuf>,
        /// The port on this machine's loopback. Another port is another origin, which
        /// starts without the page's saved key and conversations, so a port in
        /// use is an error rather than a reason to take the next one.
        #[arg(long, default_value_t = vitna_app_server::DEFAULT_PORT)]
        port: u16,
        /// The built interface: `npm run chat:build -- --mode runner` in
        /// convexityos/vitna. Defaults to a `ui` folder beside this program.
        #[arg(long)]
        ui: Option<PathBuf>,
        /// Run commands with no OS sandbox where this machine has none.
        /// Without it such a command is refused rather than run unprotected.
        #[arg(long)]
        allow_unsandboxed: bool,
        /// Print each window's address instead of opening it.
        #[arg(long)]
        no_open: bool,
        /// A folder of routine definitions (`platform/connections` in
        /// convexityos/vitna), which the page's Routines place lists with
        /// their receipts. Read only: nothing is run, written or unlocked.
        #[arg(long)]
        routines: Option<PathBuf>,
        /// The Vitna runtime's ledger (`VITNA_LEDGER` in convexityos/vitna),
        /// whose rows the Routines map prices per agent. Read only.
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// Whom the map asks for each model's public price history, by its
        /// slug alone: an https origin, or `none` to price nothing. Only with
        /// --ledger.
        #[arg(long, requires = "ledger")]
        record: Option<String>,
    },
    /// Build, sign, and verify Ed25519 release manifests (vitna-release-manifest-v1)
    Release {
        #[command(subcommand)]
        sub: ReleaseCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ReleaseCommands {
    /// Hash real archive files on disk and sign a vitna-release-manifest-v1.json file
    Sign {
        /// Semver version string (for example 0.2.0 or 0.2.0-beta.1).
        #[arg(long)]
        version: String,
        /// Release channel: stable, beta, or nightly.
        #[arg(long, default_value = "beta")]
        channel: String,
        /// Release notes URL.
        #[arg(long)]
        release_notes_url: Option<String>,
        /// Asset spec `TARGET_TRIPLE=ARCHIVE_PATH` (repeatable).
        #[arg(long = "asset", value_name = "TRIPLE=PATH", required = true)]
        assets: Vec<String>,
        /// Output path for the signed manifest JSON.
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify a release manifest's Ed25519 publisher signature and optional on-disk archives
    Verify {
        /// Path to the release manifest JSON file.
        manifest_file: PathBuf,
        /// Expected 64-char hex Ed25519 publisher public key.
        #[arg(long)]
        key: Option<String>,
        /// Directory containing the packaged release archives to hash and check.
        #[arg(long)]
        assets_dir: Option<PathBuf>,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AuthCommands {
    /// Show OS-keychain credential readiness for supported providers
    Status {
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Store a provider API key in the OS keychain (reads the key from stdin, never CLI argv)
    Set {
        /// Provider name: anthropic or openai
        provider: String,
    },
    /// Remove a provider API key from the OS keychain
    Clear {
        /// Provider name: anthropic or openai
        provider: String,
    },
}

#[derive(Subcommand, Debug)]
enum SkillsCommands {
    /// List discovered workspace skills and any refused paths
    List {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Verify workspace skills against a SHA-256 lockfile, or write/update one
    Verify {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Path to the skill lock JSON file (defaults to <workspace>/.vitna/skills.lock.json).
        #[arg(long)]
        lock: Option<PathBuf>,
        /// Write or update the lockfile from currently valid discovered skills.
        #[arg(long)]
        write_lock: bool,
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ReceiptCommands {
    /// Show and verify the receipt for a specific run
    Show {
        run_id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Export receipt for a specific run with explicit redaction level
    Export {
        run_id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Redaction level: none (verbatim signed receipt), standard, or strict.
        #[arg(long, default_value = "standard")]
        redaction: String,
        /// Optional output file path (defaults to stdout).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Doctor) => {
            run_doctor().await?;
        }
        Some(Commands::Verify { receipt_file, key }) => {
            run_verify(&receipt_file, key.as_deref())?;
        }
        Some(Commands::Run {
            task,
            workspace,
            auto_approve,
            verify_cmd,
            allow_unsandboxed,
            runtime_url,
            lane,
            provider,
            model,
            base_url,
            checkpoint,
            mcp_http,
            mcp_stdio,
            allow_egress,
            sandbox_guarantee,
            sandbox_image,
            mandate,
            store,
            ..
        }) => {
            let has_live_flags = runtime_url.is_some()
                || lane.is_some()
                || provider.is_some()
                || model.is_some()
                || base_url.is_some()
                || checkpoint.is_some();
            if has_live_flags {
                if let Err(e) = commands::run_live_turn(commands::LiveRunOptions {
                    task: &task,
                    workspace: &workspace,
                    store_arg: store.as_deref(),
                    runtime_url,
                    lane,
                    provider,
                    model,
                    base_url,
                    checkpoint,
                    mcp_http,
                    mcp_stdio,
                    allow_egress,
                    sandbox_guarantee,
                    sandbox_image,
                    mandate,
                    auto_approve,
                    verify_cmd,
                    allow_unsandboxed,
                })
                .await
                {
                    eprintln!("vitna run: {e}");
                    std::process::exit(1);
                }
            } else if !mcp_http.is_empty()
                || !mcp_stdio.is_empty()
                || !allow_egress.is_empty()
                || sandbox_guarantee.is_some()
                || sandbox_image.is_some()
                || mandate.is_some()
            {
                eprintln!(
                    "vitna run: --mcp-http, --mcp-stdio, --allow-egress, --sandbox-guarantee, --sandbox-image, and --mandate require a model route (--runtime-url/--lane or --provider/--model)."
                );
                std::process::exit(1);
            } else {
                run_task_cli(
                    &task,
                    &workspace,
                    store.as_deref(),
                    auto_approve,
                    verify_cmd,
                    allow_unsandboxed,
                )
                .await?;
            }
        }
        Some(Commands::Sessions { store, json }) => {
            if let Err(e) = commands::run_list_sessions(store.as_deref(), json) {
                eprintln!("vitna sessions: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Resume {
            session_id,
            store,
            prompt,
            endpoint,
            json,
        }) => {
            if let Err(e) = commands::run_resume(
                session_id.as_deref(),
                store.as_deref(),
                prompt.as_deref(),
                endpoint.as_deref(),
                json,
            )
            .await
            {
                eprintln!("vitna resume: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Review {
            path_or_ref,
            workspace,
            json,
        }) => {
            if let Err(e) = commands::run_review(path_or_ref.as_deref(), &workspace, json) {
                eprintln!("vitna review: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Apply {
            change_set_id,
            workspace,
            source,
            check,
        }) => {
            if let Err(e) =
                commands::run_apply(&change_set_id, &workspace, source.as_deref(), check)
            {
                eprintln!("vitna apply: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Serve {
            endpoint,
            store,
            runtime_url,
            lane,
            provider,
            model,
            base_url,
            checkpoint,
            mcp_http,
            mcp_stdio,
            allow_egress,
            sandbox_guarantee,
            sandbox_image,
            mandate,
        }) => {
            if let Err(e) = serve(
                endpoint,
                store,
                runtime_url,
                lane,
                provider,
                model,
                base_url,
                checkpoint,
                mcp_http,
                mcp_stdio,
                allow_egress,
                sandbox_guarantee,
                sandbox_image,
                mandate,
            )
            .await
            {
                // One line naming what failed, as vitna-coded prints it. A
                // daemon that dies quietly looks, from a client, like one that
                // was never started.
                eprintln!("vitna serve: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Auth { sub }) => match sub {
            AuthCommands::Status { json } => {
                if let Err(e) = commands::run_auth_status(json) {
                    eprintln!("vitna auth status: {e}");
                    std::process::exit(1);
                }
            }
            AuthCommands::Set { provider } => {
                if let Err(e) = commands::run_auth_set(&provider) {
                    eprintln!("vitna auth set: {e}");
                    std::process::exit(1);
                }
            }
            AuthCommands::Clear { provider } => {
                if let Err(e) = commands::run_auth_clear(&provider) {
                    eprintln!("vitna auth clear: {e}");
                    std::process::exit(1);
                }
            }
        },
        Some(Commands::Skills { sub }) => match sub {
            SkillsCommands::List { workspace, json } => {
                if let Err(e) = commands::run_skills_list(&workspace, json) {
                    eprintln!("vitna skills list: {e}");
                    std::process::exit(1);
                }
            }
            SkillsCommands::Verify {
                workspace,
                lock,
                write_lock,
                json,
            } => {
                if let Err(e) =
                    commands::run_skills_verify(&workspace, lock.as_deref(), write_lock, json)
                {
                    eprintln!("vitna skills verify: {e}");
                    std::process::exit(1);
                }
            }
        },
        Some(Commands::App {
            folder,
            port,
            ui,
            allow_unsandboxed,
            no_open,
            routines,
            ledger,
            record,
        }) => {
            // The ledger, and whom its prices are asked of: vitna.ai unless
            // the operator names another record, or none.
            let priced = ledger.map(|path| {
                let record = match record.as_deref() {
                    Some("none") => None,
                    Some(origin) => Some(origin.to_string()),
                    None => Some(vitna_app_server::DEFAULT_RECORD.to_string()),
                };
                (path, record)
            });
            if let Err(e) =
                run_app(folder, port, ui, allow_unsandboxed, no_open, routines, priced).await
            {
                eprintln!("vitna app: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Release { sub }) => match sub {
            ReleaseCommands::Sign {
                version,
                channel,
                release_notes_url,
                assets,
                out,
            } => {
                if let Err(e) = commands::run_release_sign(
                    &version,
                    &channel,
                    release_notes_url.as_deref(),
                    &assets,
                    &out,
                ) {
                    eprintln!("vitna release sign: {e}");
                    std::process::exit(1);
                }
            }
            ReleaseCommands::Verify {
                manifest_file,
                key,
                assets_dir,
                json,
            } => {
                if let Err(e) = commands::run_release_verify(
                    &manifest_file,
                    key.as_deref(),
                    assets_dir.as_deref(),
                    json,
                ) {
                    eprintln!("vitna release verify: {e}");
                    std::process::exit(1);
                }
            }
        },
        Some(Commands::Receipt { sub }) => match sub {
            ReceiptCommands::Show {
                run_id,
                workspace,
                key,
                json,
            } => {
                if let Err(e) =
                    commands::run_receipt_show(&run_id, &workspace, key.as_deref(), json)
                {
                    eprintln!("vitna receipt show: {e}");
                    std::process::exit(1);
                }
            }
            ReceiptCommands::Export {
                run_id,
                workspace,
                redaction,
                out,
            } => {
                if let Err(e) = commands::run_receipt_export(
                    &run_id,
                    &workspace,
                    &redaction,
                    out.as_deref(),
                ) {
                    eprintln!("vitna receipt export: {e}");
                    std::process::exit(1);
                }
            }
        },
        None => {
            println!("Vitna Code. Run 'vitna --help' for available commands.");
        }
    }

    Ok(())
}

/// Starts the daemon in the foreground, through the same code as vitna-coded.
///
/// This opened a store beside the current folder, printed "Vitna daemon
/// listening on local session channel" and bound nothing, so every client that
/// probed the declared endpoint found no daemon. The release scripts ship
/// `vitna` and not `vitna-coded`, so for an installed copy this is the door.
#[allow(clippy::too_many_arguments)]
async fn serve(
    endpoint: Option<String>,
    store: Option<PathBuf>,
    runtime_url: Option<String>,
    lane: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    checkpoint: Option<PathBuf>,
    mcp_http: Vec<String>,
    mcp_stdio: Vec<String>,
    allow_egress: Vec<String>,
    sandbox_guarantee: Option<String>,
    sandbox_image: Option<String>,
    mandate: Option<PathBuf>,
) -> Result<(), String> {
    vitna_daemon::launch::logging();
    let launch = vitna_daemon::launch::Launch::resolve(endpoint, store)?
        .with_runtime(runtime_url, lane)?
        .with_direct_provider(provider, model, base_url)?
        .with_checkpoint(checkpoint)?
        .with_mcp_servers(mcp_http, mcp_stdio)?
        .with_egress_allowlist(allow_egress)?
        .with_sandbox(sandbox_guarantee, sandbox_image)?
        .with_mandate(mandate)?;
    let announce = launch.endpoint.clone();
    vitna_daemon::launch::run(&launch, move || {
        // Printed once the endpoint is bound, so it is true when it is read.
        println!("Vitna daemon listening on {announce}. Press Ctrl+C to exit.");
    })
    .await
}

/// A path as a person writes it, without the `\\?\` Windows puts in front of
/// a resolved one.
fn shown(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
}

/// Serves a folder to the Vitna Code interface on localhost (ADR-0006) and
/// opens it in a window, until Ctrl+C.
async fn run_app(
    folder: Option<PathBuf>,
    port: u16,
    ui: Option<PathBuf>,
    allow_unsandboxed: bool,
    no_open: bool,
    routines: Option<PathBuf>,
    priced: Option<(PathBuf, Option<String>)>,
) -> Result<(), String> {
    use vitna_app_server::{AppConfig, AppServer, Commands};

    let home = vitna_daemon::launch::vitna_home()?;
    fs::create_dir_all(&home)
        .map_err(|e| format!("could not make {}: {e}", home.display()))?;
    let signing_key = vitna_daemon::key::load_or_create(&home)?;
    let journal = home.join("app").join("runner.journal");
    if let Some(parent) = journal.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("could not make {}: {e}", parent.display()))?;
    }
    let runner = vitna_runner::ProcessRunner::open_or_create(&journal)
        .map_err(|e| format!("could not open the runner's journal {}: {e}", journal.display()))?;
    let ui = ui.or_else(|| {
        let beside = std::env::current_exe().ok()?.parent()?.join("ui");
        beside.join("index.html").is_file().then_some(beside)
    });
    let has_ui = ui.is_some();

    let server = AppServer::bind(
        AppConfig {
            folder: folder.unwrap_or_else(|| PathBuf::from(".")),
            port,
            ui,
            allow_unsandboxed,
            routines,
            ledger: priced.as_ref().map(|(path, _)| path.clone()),
            record: priced.and_then(|(_, record)| record),
        },
        std::sync::Arc::new(runner),
        signing_key,
    )
    .await?;

    println!("vitna app is serving {} at {}", shown(server.folder()), server.origin());
    match server.commands() {
        Commands::Sandboxed { backend, enforcement } => {
            println!("Commands run in the {backend} sandbox ({enforcement}), with no network.")
        }
        Commands::Unsandboxed { reason } => println!(
            "Commands run WITHOUT a sandbox, as --allow-unsandboxed asked: this machine has none ({reason})."
        ),
        Commands::Refused { reason } => println!(
            "Commands are refused: this machine has no sandbox ({reason}). Start with --allow-unsandboxed to run them without one."
        ),
        Commands::NoProcess => println!("Commands are not run."),
    }
    println!("Receipts are signed with the device key {}.", server.public_key());
    if let Some(dir) = server.routines() {
        println!(
            "Routines are read from {}, and never run or written from the page.",
            shown(dir)
        );
    }
    if let Some(file) = server.ledger() {
        match server.record() {
            Some(record) => println!(
                "The runtime ledger is read from {}. The map asks {record} for each model's price history by its slug alone.",
                shown(file)
            ),
            None => println!(
                "The runtime ledger is read from {}. Nothing is priced (--record none).",
                shown(file)
            ),
        }
    }
    if !has_ui {
        println!(
            "No interface found beside this program. Build one with `npm run chat:build -- --mode runner` in convexityos/vitna and pass --ui."
        );
    }

    let launcher = server.launcher();
    let open_one = move |launcher: &vitna_app_server::Launcher| {
        let url = launcher.url();
        if no_open {
            println!("Open {url} within two minutes. It works once.");
            return;
        }
        match vitna_app_server::window::open(&url) {
            Ok(how) => println!("Opened {how}."),
            Err(e) => println!("{e}"),
        }
    };
    open_one(&launcher);
    println!("Press Enter for another window, Ctrl+C to stop.");
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            if line.is_err() {
                break;
            }
            open_one(&launcher);
        }
    });

    tokio::select! {
        served = server.serve() => served,
        _ = tokio::signal::ctrl_c() => {
            println!("Stopped.");
            Ok(())
        }
    }
}

async fn run_doctor() -> Result<(), Box<dyn std::error::Error>> {
    println!("Vitna Code Doctor - Environment Audit");
    println!("=====================================");

    // 1. Operating System
    println!("[OK] OS: {} ({})", std::env::consts::OS, std::env::consts::ARCH);

    // 2. Git
    match vitna_git_workspaces::host_git::version() {
        Some(version) => println!("[OK] Git: {}", version),
        None => println!("[WARN] Git executable not found on PATH."),
    }

    // 3. SQLite WAL support
    let temp_db = std::env::temp_dir().join("vitna_doctor_check.db");
    match vitna_store::EventStore::open(&temp_db) {
        Ok(_) => {
            println!("[OK] SQLite WAL Persistence: Operational");
            let _ = fs::remove_file(temp_db);
        }
        Err(e) => {
            println!("[FAIL] SQLite WAL Persistence error: {}", e);
        }
    }

    // 4. Device Signing Key: the one this account signs receipts with, read
    // and never made here. This used to print the public half of a key made
    // for the occasion and thrown away, which no receipt was ever signed with.
    match vitna_daemon::launch::vitna_home() {
        Ok(home) => match vitna_daemon::key::public_key_in(&home) {
            Ok(Some(public)) => {
                println!("[OK] Device Signing Key: Ed25519, in {}", home.display());
                println!("     Device Public Key: {}", public);
            }
            Ok(None) => println!(
                "[OK] Device Signing Key: none yet. The daemon makes one in {} the first time it signs.",
                home.display()
            ),
            Err(e) => println!("[FAIL] Device Signing Key: {}", e),
        },
        Err(e) => println!("[FAIL] Device Signing Key: {}", e),
    }

    // 5. OS Sandbox Backend probe
    let probe_dir = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    match vitna_runner::sandbox_status(&probe_dir, false) {
        Ok((backend, enforcement)) => {
            println!("[OK] Sandbox Backend: {backend} ({enforcement})");
        }
        Err(reason) => {
            println!("[WARN] Sandbox Backend: unavailable ({reason})");
        }
    }

    // 6. Daemon IPC Endpoint handshake probe
    let (_listening, daemon_status) = commands::probe_daemon_endpoint().await;
    println!("{daemon_status}");

    println!("\nDoctor audit complete.");
    Ok(())
}

fn run_verify(receipt_path: &str, key: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(receipt_path);
    if !path.exists() {
        eprintln!("Error: Receipt file does not exist: {}", receipt_path);
        std::process::exit(1);
    }

    let content = fs::read_to_string(path)?;
    let verified = match verify_receipt_json(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Receipt could not be read: {}", e);
            std::process::exit(1);
        }
    };

    // verify_receipt_json returns Ok for a receipt it found faults in, so the
    // verdict is report.is_valid and not the absence of an error.
    if !verified.report.is_valid {
        eprintln!("Receipt Verification FAILED:");
        for err in &verified.report.errors {
            eprintln!("  - {}", err);
        }
        std::process::exit(1);
    }

    // The signature: against the key given, which it must match, or else
    // against this account's device key, which a receipt signed on another
    // device does not match without anything being wrong with it.
    let signature = match key {
        Some(given) => {
            let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&verified.receipt, Some(given))?;
            if !report.signature_verified {
                eprintln!("Receipt Verification FAILED:");
                for err in &report.errors {
                    eprintln!("  - {}", err);
                }
                std::process::exit(1);
            }
            "verified against the key given".to_string()
        }
        None => {
            let device = vitna_daemon::launch::vitna_home()
                .ok()
                .and_then(|home| vitna_daemon::key::public_key_in(&home).ok().flatten());
            match device {
                None => "not checked: this account has no device key to check it against, and no --key was given"
                    .to_string(),
                Some(public) => {
                    let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&verified.receipt, Some(&public))?;
                    if report.signature_verified {
                        "verified against this device's key".to_string()
                    } else if report
                        .errors
                        .iter()
                        .any(|e| e.contains("different device key"))
                    {
                        "signed by a different device key (pass --key with the signer's public key)"
                            .to_string()
                    } else {
                        "not this device's key: signed on another device, or changed since it was signed. Check it with --key and the signer's public key".to_string()
                    }
                }
            }
        }
    };

    let r = &verified.receipt;
    println!("Receipt Checks Passed");
    println!("================================");
    println!("Run ID:            {}", r.run_id);
    println!("Session ID:        {}", r.session_id);
    println!("Schema:            {}", r.schema_version);
    println!("Model:             {} ({})", r.model_selection.model_sku, r.model_selection.provider);
    println!("Isolation Label:   {}", r.isolation_label);
    println!("Completion State:  {}", r.completion_state);
    println!("Evidence Count:    {}", r.evidence_items.len());
    println!("Files Modified:    {}", r.changeset.files_modified.len());
    println!("Device Signature:  {}", signature);

    Ok(())
}

async fn run_task_cli(
    task: &str,
    workspace: &Path,
    store_arg: Option<&Path>,
    auto_approve: bool,
    verify_cmd: Option<String>,
    allow_unsandboxed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Vitna Code Task Execution");
    println!("-------------------------");
    println!("Task:      {}", task);
    println!("Workspace: {}", workspace.display());

    let db_path = match store_arg {
        Some(p) => p.to_path_buf(),
        None => workspace.join(".vitna").join("store.db"),
    };
    let daemon = DaemonServer::open_default(&db_path)?;

    let session = daemon.create_session(workspace)?;
    println!("Session:   {}", session.session_id);

    println!("Starting turn execution...");
    let receipt = daemon
        .run_task(
            &session.session_id,
            task,
            auto_approve,
            verify_cmd,
            allow_unsandboxed,
        )
        .await?;

    println!("\nTask Execution Complete!");
    println!("========================");
    println!("Run ID:           {}", receipt.run_id);
    println!("Completion State: {}", receipt.completion_state);
    println!("Files Modified:   {}", receipt.changeset.files_modified.len());
    for f in &receipt.changeset.files_modified {
        println!("  - {}", f.path);
    }
    println!("Evidence Captured: {}", receipt.evidence_items.len());
    for ev in &receipt.evidence_items {
        println!("  - [{}] {}", ev.grade, ev.description);
    }
    println!("Device Signature: {}", receipt.device_signature);
    println!("Receipt Path:     .vitna/receipts/{}.json", receipt.run_id);

    Ok(())
}
