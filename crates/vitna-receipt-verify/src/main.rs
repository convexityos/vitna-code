//! Offline receipt and cross-product provenance verification CLI for Vitna Code.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use vitna_receipt_verify::{verify_provenance_bundle, ProvenanceAuditOptions};

#[derive(Parser, Debug)]
#[command(name = "vitna-receipt-verify")]
#[command(about = "Verify a Vitna Code receipt and optional checkpoint / DataEnvelope provenance offline", long_about = None)]
struct Args {
    /// Path to the vitna-run-receipt-v1 JSON file
    receipt_file: PathBuf,

    /// Optional Ed25519 public key (64-char hex) to verify the device signature
    #[arg(long)]
    public_key: Option<String>,

    /// Fail closed unless the device signature verified against --public-key
    #[arg(long)]
    require_signature: bool,

    /// Optional local vitna-anchor checkpoint file or directory to SHA-256 cross-check
    /// against receipt.model_selection.policy_digest
    #[arg(long)]
    checkpoint_path: Option<PathBuf>,

    /// Optional Convexity API DataEnvelope JSON file(s) to audit for freshness
    #[arg(long = "envelope")]
    envelopes: Vec<PathBuf>,

    /// Fail closed if any supplied DataEnvelope reports sync_status="stale"
    #[arg(long)]
    strict_freshness: bool,

    /// Print machine-readable JSON output for CI pipelines
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let opts = ProvenanceAuditOptions {
        receipt_path: args.receipt_file.clone(),
        public_key_hex: args.public_key,
        require_signature: args.require_signature,
        checkpoint_path: args.checkpoint_path,
        envelope_paths: args.envelopes,
        strict_freshness: args.strict_freshness,
    };

    match verify_provenance_bundle(&opts) {
        Ok(audit) => {
            if args.json {
                match serde_json::to_string_pretty(&audit) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("Failed to serialize JSON report: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                println!("Verifying Vitna Code receipt: {}", args.receipt_file.display());
                println!("Receipt Verification Report:");
                println!("  Run ID:            {}", audit.receipt.run_id);
                println!("  Schema Version:    {}", audit.receipt.schema_version);
                println!("  Canonical SHA-256: {}", audit.canonical_sha256);
                println!("  Isolation Label:   {}", audit.receipt.isolation_label);
                println!("  Completion State:  {}", audit.receipt.completion_state);
                println!("  Evidence Items:    {}", audit.receipt.evidence_count);
                println!("  Signature Checked: {}", audit.receipt.signature_verified);
                if let Some(ref ckpt) = audit.checkpoint {
                    println!(
                        "  Checkpoint Match:  {} ({})",
                        if ckpt.matched { "MATCHED" } else { "MISMATCH" },
                        ckpt.local_sha256.as_deref().unwrap_or("unhashed")
                    );
                }
                for env in &audit.envelopes {
                    println!(
                        "  DataEnvelope:      {} -> {} ({})",
                        env.source_file,
                        if env.is_valid { "OK" } else { "FAILED" },
                        env.citation
                    );
                }
                println!(
                    "  Status:            {}",
                    if audit.overall_valid { "VALID" } else { "INVALID" }
                );

                if !audit.overall_valid {
                    println!("\nVerification Errors:");
                    for err in &audit.errors {
                        println!("  - {}", err);
                    }
                } else {
                    println!("\nIntegrity check PASSED.");
                }
            }

            if audit.overall_valid {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("Verification execution failed: {}", e);
            ExitCode::FAILURE
        }
    }
}