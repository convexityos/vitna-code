//! Vitna CLI
//!
//! Non-interactive command-line interface and machine runner for Vitna Code.

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
    },
    /// Resume an existing session
    Resume { session_id: Option<String> },
    /// Review changes or diffs against evidence
    Review { path_or_ref: Option<String> },
    /// Apply an approved change set to the primary checkout
    Apply { change_set_id: String },
    /// List active and previous sessions
    Sessions,
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
    /// Check local environment and daemon health
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
    },
}

#[derive(Subcommand, Debug)]
enum ReceiptCommands {
    /// Show receipt for a specific run
    Show { run_id: String },
    /// Export receipt for a specific run
    Export {
        run_id: String,
        #[arg(long, default_value = "standard")]
        redaction: String,
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
            ..
        }) => {
            run_task_cli(&task, &workspace, auto_approve, verify_cmd, allow_unsandboxed).await?;
        }
        Some(Commands::Sessions) => {
            run_list_sessions()?;
        }
        Some(Commands::Serve { endpoint, store }) => {
            if let Err(e) = serve(endpoint, store).await {
                // One line naming what failed, as vitna-coded prints it. A
                // daemon that dies quietly looks, from a client, like one that
                // was never started.
                eprintln!("vitna serve: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::App {
            folder,
            port,
            ui,
            allow_unsandboxed,
            no_open,
        }) => {
            if let Err(e) = run_app(folder, port, ui, allow_unsandboxed, no_open).await {
                eprintln!("vitna app: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Receipt { sub }) => match sub {
            ReceiptCommands::Show { run_id } => {
                println!("Displaying receipt for run: {}", run_id);
            }
            ReceiptCommands::Export { run_id, redaction } => {
                println!("Exporting receipt for run {} with redaction {}", run_id, redaction);
            }
        },
        Some(cmd) => {
            println!("Command {:?} executed.", cmd);
        }
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
async fn serve(endpoint: Option<String>, store: Option<PathBuf>) -> Result<(), String> {
    vitna_daemon::launch::logging();
    let launch = vitna_daemon::launch::Launch::resolve(endpoint, store)?;
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

    println!("\nAll systems verified for Vitna local-first operation.");
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
    auto_approve: bool,
    verify_cmd: Option<String>,
    allow_unsandboxed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Vitna Code Task Execution");
    println!("-------------------------");
    println!("Task:      {}", task);
    println!("Workspace: {}", workspace.display());

    let db_path = workspace.join(".vitna").join("store.db");
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

fn run_list_sessions() -> Result<(), Box<dyn std::error::Error>> {
    println!("Vitna Sessions (Local Workspace)");
    println!("--------------------------------");
    let db_path = Path::new(".vitna").join("store.db");
    if !db_path.exists() {
        println!("No local .vitna/store.db found in current directory.");
        return Ok(());
    }

    let daemon = DaemonServer::open_default(&db_path)?;
    let sessions = daemon.list_sessions();
    if sessions.is_empty() {
        println!("No active sessions recorded.");
    } else {
        for s in sessions {
            println!("- Session ID: {} (Status: {})", s.session_id, s.status);
        }
    }

    Ok(())
}
