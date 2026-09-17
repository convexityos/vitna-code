//! Vitna CLI
//!
//! Non-interactive command-line interface and machine runner for Vitna Code.

use clap::{Parser, Subcommand};

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
        #[arg(long, default_value = "build")]
        mode: String,
        #[arg(long)]
        jsonl: bool,
    },
    /// Resume an existing session
    Resume { session_id: Option<String> },
    /// Review changes or diffs against evidence
    Review { path_or_ref: Option<String> },
    /// Apply an approved change set to the primary checkout
    Apply { change_set_id: String },
    /// Receipt operations
    Receipt {
        #[command(subcommand)]
        sub: ReceiptCommands,
    },
    /// Verify an exported receipt file independently
    Verify { receipt_file: String },
    /// Check local environment and daemon health
    Doctor,
    /// Daemon status
    Daemon {
        #[arg(long)]
        status: bool,
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

    match &cli.command {
        Some(Commands::Doctor) => {
            println!("Vitna Code (Phase 0A prototype)");
            println!("Local daemon status: Offline (Phase 0A)");
            println!("Platform verification: Ready");
        }
        Some(cmd) => {
            println!("Command {:?} acknowledged. Daemon connectivity required.", cmd);
        }
        None => {
            println!("Vitna Code. Run 'vitna --help' for available commands.");
        }
    }

    Ok(())
}
