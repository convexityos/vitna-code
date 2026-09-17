//! Offline receipt verification CLI tool for Vitna Code.

use clap::Parser;
use std::process::ExitCode;
use vitna_receipt_verify::ReceiptVerifier;

#[derive(Parser, Debug)]
#[command(name = "vitna-receipt-verify")]
#[command(about = "Verify a Vitna Code receipt offline", long_about = None)]
struct Args {
    /// Path to the vitna-run-receipt-v1 JSON file
    receipt_file: String,

    /// Optional public key (hex) to verify the device signature
    #[arg(long)]
    public_key: Option<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    println!("Verifying Vitna Code receipt: {}", args.receipt_file);

    match ReceiptVerifier::verify_file(&args.receipt_file, args.public_key.as_deref()) {
        Ok(report) => {
            println!("Receipt Verification Report:");
            println!("  Run ID:           {}", report.run_id);
            println!("  Schema Version:   {}", report.schema_version);
            println!("  Isolation Label:  {}", report.isolation_label);
            println!("  Completion State: {}", report.completion_state);
            println!("  Evidence Items:   {}", report.evidence_count);
            println!("  Status:           {}", if report.is_valid { "VALID" } else { "INVALID" });

            if !report.is_valid {
                println!("\nVerification Errors:");
                for err in report.errors {
                    println!("  - {}", err);
                }
                ExitCode::FAILURE
            } else {
                println!("\nIntegrity check PASSED.");
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("Verification execution failed: {}", e);
            ExitCode::FAILURE
        }
    }
}