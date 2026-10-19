//! A run that calls no model signs a receipt that names none.
//!
//! `DaemonServer::run_task` asks no model anything: it picks its one edit by
//! looking for a keyword in the prompt. Its receipts used to name
//! `claude-3-7-sonnet` from a provider called `fake`, so every `vitna run`
//! signed, under the device key, a statement that a model ran when none had.
//!
//! The expected values are written out here rather than read from the daemon,
//! so this cannot pass by agreeing with whatever the daemon happens to say.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use vitna_daemon::DaemonServer;
use vitna_receipts::VitnaRunReceiptV1;
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

/// No provider served the run.
const PROVIDER: &str = "none";
/// What stood in for a model: the daemon's keyword match.
const SKU: &str = "keyword-stub";

fn workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitna_no_model_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create workspace");
    dir
}

/// Runs `prompt` through the real `run_task` and returns the receipt as it was
/// written to `.vitna/receipts/`, the copy `vitna verify` and the terminal
/// read, once its signature has been checked.
async fn signed_receipt(tag: &str, prompt: &str) -> VitnaRunReceiptV1 {
    let ws = workspace(tag);
    let key = vitna_receipts::generate_signing_key();
    let verifying_key = key.verifying_key();
    let daemon = DaemonServer::new(
        EventStore::open_in_memory().expect("open store"),
        Arc::new(Mutex::new(
            FakeRunner::new(ws.join("runner.journal")).expect("open runner"),
        )),
        key,
    );
    let session = daemon.create_session(&ws).expect("create session");
    let returned = daemon
        .run_task(&session.session_id, prompt, true, None, false)
        .await
        .expect("the run completes");

    let path = ws
        .join(".vitna")
        .join("receipts")
        .join(format!("{}.json", returned.run_id));
    let written: VitnaRunReceiptV1 =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the receipt is written"))
            .expect("the receipt parses");
    assert_eq!(written, returned, "the receipt on disk is the one the run returned");
    assert!(
        written.verify_signature(&verifying_key).expect("verify"),
        "the device signature covers the receipt, model_selection included"
    );

    drop(daemon);
    let _ = std::fs::remove_dir_all(&ws);
    written
}

#[tokio::test]
async fn a_keyword_run_names_no_model() {
    let receipt = signed_receipt("keyword", "add a notes file").await;

    // The keyword path, not the quiet one: the stub's edit is in the receipt.
    assert!(
        receipt
            .changeset
            .files_modified
            .iter()
            .any(|f| f.path == "vitna_output.txt"),
        "expected the keyword stub's edit, got {:?}",
        receipt.changeset.files_modified
    );
    assert_eq!(receipt.model_selection.provider, PROVIDER);
    assert_eq!(receipt.model_selection.model_sku, SKU);
}

#[tokio::test]
async fn a_run_with_no_keyword_names_no_model_either() {
    let receipt = signed_receipt("quiet", "look around").await;

    assert!(receipt.changeset.files_modified.is_empty());
    assert_eq!(receipt.model_selection.provider, PROVIDER);
    assert_eq!(receipt.model_selection.model_sku, SKU);
}
