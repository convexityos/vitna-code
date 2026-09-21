//! The numbering contract between a run's event log and anything that
//! subscribes to it.
//!
//! `SubscribeEvents.resume_after_sequence` is a proto3 scalar, so an unset
//! field and a literal 0 are indistinguishable on the wire. A client opening
//! its first subscription therefore has no way to say "from the beginning"
//! other than to send 0, and "resume AFTER 0" means "everything" only while
//! nothing is numbered 0.
//!
//! The engine numbered its first event 0 until this file existed, which made
//! every run's opening event unreachable through a first subscription. What
//! makes that worth a test rather than a one line fix is how it hides: a
//! subscriber detects a gap by comparing an arriving sequence against the last
//! one it applied, so it needs a predecessor to measure against, and the one
//! event whose absence can never raise a gap is the first. The stream would
//! have looked healthy and started one event late, forever.
//!
//! What these tests do not check: that any daemon actually honours
//! `resume_after_sequence` when serving a subscription. No daemon serves one
//! yet. They pin the producer's numbering, which is the half that has to be
//! right before a subscription is built on it.

use std::fs;
use std::sync::{Arc, Mutex};
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine};
use vitna_receipts::generate_signing_key;
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

const RUN_ID: &str = "run-sequence-contract";

/// Records `count` events on a fresh engine and returns their sequences, in
/// the order the store gives them back.
fn recorded_sequences(count: usize) -> Vec<u64> {
    let temp_dir = std::env::temp_dir().join(format!(
        "vitna_sequence_contract_{}_{}",
        std::process::id(),
        count
    ));
    fs::create_dir_all(&temp_dir).expect("create test workspace");

    let store = Arc::new(Mutex::new(
        EventStore::open_in_memory().expect("open store"),
    ));
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(temp_dir.join("runner.journal")).expect("open fake runner"),
    ));

    let config = OrchestrationConfig {
        session_id: "sess-sequence-contract".to_string(),
        run_id: RUN_ID.to_string(),
        workspace_root: temp_dir,
        auto_approve: true,
        model_sku: "test-sku".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: None,
        allow_unsandboxed: false,
    };

    let mut engine =
        OrchestrationEngine::new(config, store.clone(), runner, generate_signing_key());

    for i in 0..count {
        engine
            .record_event(
                vitna_protocol::type_url::event::DIAGNOSTIC,
                &serde_json::json!({ "message": format!("event {i}") }),
            )
            .expect("record event");
    }

    let store = store.lock().expect("lock store");
    store
        .get_events(RUN_ID, 0)
        .expect("read events back")
        .into_iter()
        .map(|e| e.sequence)
        .collect()
}

#[test]
fn a_runs_first_event_is_numbered_one() {
    let sequences = recorded_sequences(1);

    assert_eq!(
        sequences,
        vec![vitna_protocol::FIRST_EVENT_SEQUENCE],
        "the first event a run records must carry FIRST_EVENT_SEQUENCE"
    );
    assert_eq!(
        vitna_protocol::FIRST_EVENT_SEQUENCE,
        1,
        "FIRST_EVENT_SEQUENCE is 1 because resume_after_sequence cannot \
         distinguish an unset field from a literal 0"
    );
}

/// The property the constant exists for, stated as a subscription would apply
/// it. `resume_after_sequence: 0` selects `sequence > 0`.
#[test]
fn resume_after_zero_delivers_the_runs_first_event() {
    let sequences = recorded_sequences(3);
    assert_eq!(sequences.len(), 3, "three events were recorded");

    let delivered: Vec<u64> = sequences.iter().copied().filter(|s| *s > 0).collect();

    assert_eq!(
        delivered, sequences,
        "resuming after 0 dropped an event. Every recorded sequence must be \
         strictly greater than 0, or a first subscription starts late."
    );

    let first = *sequences.first().expect("at least one event");
    assert!(
        first > 0,
        "the run's first event is numbered {first}, so a subscription resuming \
         after 0 can never receive it"
    );
}

/// A subscriber applies an event only when it continues the one before it, so
/// the producer owes it a contiguous run with no holes.
#[test]
fn sequences_are_contiguous_from_the_first() {
    let sequences = recorded_sequences(5);

    let expected: Vec<u64> = (0..5)
        .map(|i| vitna_protocol::FIRST_EVENT_SEQUENCE + i)
        .collect();

    assert_eq!(
        sequences, expected,
        "events must be numbered consecutively from FIRST_EVENT_SEQUENCE; a \
         hole stalls a subscriber, which treats it as a gap and resubscribes"
    );
}

/// The other half of the contract lives in TypeScript, and nothing but a test
/// like this compares them. If the desktop client ever opens a subscription at
/// something other than 0, the reasoning behind `FIRST_EVENT_SEQUENCE` has
/// changed and this file should be revisited rather than quietly kept.
#[test]
fn the_desktop_client_still_opens_subscriptions_at_zero() {
    let client_ts = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/orchestration sits two levels below the repository root")
        .join("apps/vitna-desktop/src/client/client.ts");

    let source = std::fs::read_to_string(&client_ts)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", client_ts.display()));

    let opening = source
        .lines()
        .map(str::trim)
        .find(|line| line.contains("this.subscribe(") && !line.starts_with("*"))
        .expect(
            "client.ts no longer opens a subscription through this.subscribe(); \
             the numbering contract needs rechecking against whatever replaced it",
        );

    assert!(
        opening.contains(", 0)"),
        "the desktop client opens its first subscription with {opening:?}, not 0. \
         FIRST_EVENT_SEQUENCE is 1 precisely because that argument is 0, so if it \
         has moved, the two sides no longer agree about where a stream starts."
    );
}
