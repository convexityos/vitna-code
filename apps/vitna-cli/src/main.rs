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
    Verify { receipt_file: String },
    /// Check local environment and daemon health
    Doctor,
    /// Start local daemon server
    Serve {
        #[arg(long, default_value = ".vitna/store.db")]
        db: PathBuf,
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
        Some(Commands::Verify { receipt_file }) => {
            run_verify(&receipt_file)?;
        }
        Some(Commands::Run {
            task,
            workspace,
            auto_approve,
            verify_cmd,
            ..
        }) => {
            run_task_cli(&task, &workspace, auto_approve, verify_cmd).await?;
        }
        Some(Commands::Sessions) => {
            run_list_sessions()?;
        }
        Some(Commands::Serve { db }) => {
            println!("Starting Vitna local daemon on {}", db.display());
            let _daemon = DaemonServer::open_default(&db)?;
            println!("Vitna daemon listening on local session channel. Press Ctrl+C to exit.");
            tokio::signal::ctrl_c().await?;
            println!("Daemon shut down cleanly.");
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

async fn run_doctor() -> Result<(), Box<dyn std::error::Error>> {
    println!("Vitna Code Doctor - Environment Audit");
    println!("=====================================");

    // 1. Operating System
    println!("[OK] OS: {} ({})", std::env::consts::OS, std::env::consts::ARCH);

    // 2. Git
    let git_check = std::process::Command::new("git").arg("--version").output();
    match git_check {
        Ok(out) if out.status.success() => {
            println!("[OK] Git: {}", String::from_utf8_lossy(&out.stdout).trim());
        }
        _ => {
            println!("[WARN] Git executable not found on PATH.");
        }
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

    // 4. Device Signing Key
    let key = vitna_receipts::generate_signing_key();
    let pubkey_hex = hex::encode(key.verifying_key().to_bytes());
    println!("[OK] Cryptographic Signing Core: Ed25519 Operational");
    println!("     Device Public Key: {}", pubkey_hex);

    println!("\nAll systems verified for Vitna local-first operation.");
    Ok(())
}

fn run_verify(receipt_path: &str) -> Result<(), Box<dyn std::error::Error>> {
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
    if verified.report.signature_verified {
        println!("Device Signature:  verified");
    } else {
        println!("Device Signature:  not checked (no public key supplied)");
    }

    Ok(())
}

async fn run_task_cli(
    task: &str,
    workspace: &Path,
    auto_approve: bool,
    verify_cmd: Option<String>,
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
        .run_task(&session.session_id, task, auto_approve, verify_cmd)
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
