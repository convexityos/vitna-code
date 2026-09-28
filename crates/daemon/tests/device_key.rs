//! One account is one signer, and its receipts check against the key it keeps.
//!
//! `DaemonServer::open_default` used to make a fresh signing key on every
//! start and keep it nowhere, so no receipt could be checked against anything
//! once the daemon that signed it had stopped. The key is now made once, in
//! the account's Vitna folder, and every daemon that opens reads it back,
//! whichever store it opens. It is never kept beside a store: `vitna run` keeps
//! its store inside the project it runs in, where a key file could be committed
//! with everything else.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use vitna_daemon::{key, DaemonServer};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

fn fresh(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitna_device_key_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a folder for the test");
    dir
}

#[test]
fn two_daemons_on_two_stores_sign_with_the_one_key_the_account_keeps() {
    let dir = fresh("two_stores");
    let home = dir.join("home").join(".vitna");
    let first = DaemonServer::open_with_key_in(dir.join("a").join("daemon.db"), &home).expect("the first daemon opens");
    let second = DaemonServer::open_with_key_in(dir.join("project").join(".vitna").join("store.db"), &home)
        .expect("the second daemon opens");
    assert_eq!(first.device_public_key(), second.device_public_key(), "two daemons of one account are two signers");
    assert!(home.join(key::KEY_FILE).exists(), "the key is not in the account's folder");
    assert!(
        !dir.join("project").join(".vitna").join(key::KEY_FILE).exists(),
        "a key was written inside the project, beside its store"
    );
    assert!(!dir.join("a").join(key::KEY_FILE).exists(), "a key was written beside a store");
    assert_eq!(key::public_key_in(&home).expect("read"), Some(first.device_public_key()));
    drop((first, second));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_receipt_checks_against_the_key_the_account_keeps() {
    let dir = fresh("receipt");
    let home = dir.join(".vitna");
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create the workspace");
    let daemon = DaemonServer::new(
        EventStore::open_in_memory().expect("open store"),
        Arc::new(Mutex::new(FakeRunner::new(dir.join("runner.journal")).expect("open runner"))),
        key::load_or_create(&home).expect("the key is made"),
    );
    let session = daemon.create_session(&workspace).expect("create session");
    let receipt = daemon
        .run_task(&session.session_id, "add a notes file", true, None, false)
        .await
        .expect("the run completes");

    // Checked the way anyone holding the receipt checks it: against the public
    // half the account publishes, read back from the key file, not taken from
    // the daemon that signed.
    let public = key::public_key_in(&home).expect("read").expect("the key exists");
    let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&receipt, Some(&public)).expect("verify");
    assert!(report.signature_verified, "the receipt does not check against the account's key: {:?}", report.errors);

    // And a key that is not the account's does not pass for it.
    let other = hex::encode(vitna_receipts::generate_signing_key().verifying_key().to_bytes());
    let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&receipt, Some(&other)).expect("verify");
    assert!(!report.signature_verified, "a receipt checked against another key passed");

    drop(daemon);
    let _ = std::fs::remove_dir_all(&dir);
}
