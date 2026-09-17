//! vitna-receipt-verify CLI binary

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "vitna-receipt-verify")]
#[command(about = "Verify a Vitna Code receipt offline")]
struct Args {
    receipt_file: String,
}

fn main() {
    let args = Args::parse();
    println!("Verifying receipt: {}", args.receipt_file);
}