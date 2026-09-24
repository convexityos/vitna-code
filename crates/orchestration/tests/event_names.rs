//! Every event this engine records is either a message `events.proto`
//! declares, or one of its own audit names. There is no third kind.
//!
//! The rule is enforced in `record_event`, the one place every event passes
//! through, rather than at each call site, because the call site that skipped
//! the convention is exactly the one that would skip a check written beside it.
//!
//! What a third kind costs: the engine recorded `vitna.v1.ToolStarted` for
//! months, while the declared prefix is `type.vitna.ai/vitna.protocol.v1.`, so
//! a client rejected every event as an unknown type. Correcting only the
//! prefix would have been worse than leaving it: the payloads shared no fields
//! with the declared messages, so the events would have decoded "successfully"
//! with everything empty, and a client's approval checks would have gone
//! quietly dead while every screen still looked right.

use std::fs;
use std::sync::{Arc, Mutex};
use vitna_orchestration::{audit, OrchestrationConfig, OrchestrationEngine};
use vitna_receipts::generate_signing_key;
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

fn engine() -> OrchestrationEngine {
    let dir = std::env::temp_dir().join(format!("vitna_event_names_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create workspace");
    let store = Arc::new(Mutex::new(
        EventStore::open_in_memory().expect("open store"),
    ));
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(dir.join("runner.journal")).expect("open runner"),
    ));
    let config = OrchestrationConfig {
        session_id: "sess-names".to_string(),
        run_id: "run-names".to_string(),
        workspace_root: dir,
        auto_approve: true,
        model_sku: "test-sku".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: None,
        allow_unsandboxed: false,
    };
    OrchestrationEngine::new(config, store, runner, generate_signing_key())
}

#[test]
fn a_declared_event_is_recorded() {
    let mut e = engine();
    assert!(e
        .record_event(
            vitna_protocol::type_url::event::TOOL_STARTED,
            &serde_json::json!({}),
        )
        .is_ok());
}

#[test]
fn an_audit_name_is_recorded() {
    let mut e = engine();
    for name in audit::ALL {
        assert!(
            e.record_event(name, &serde_json::json!({})).is_ok(),
            "{name} should be recordable"
        );
    }
}

/// The exact shape the engine used to record, and the one someone reaching for
/// a "quick fix" would reintroduce.
#[test]
fn the_old_ad_hoc_name_is_refused() {
    let mut e = engine();
    let err = e
        .record_event("vitna.v1.ToolStarted", &serde_json::json!({}))
        .expect_err("an undeclared, non-audit name must not be recorded");
    assert!(err.contains("neither"), "the refusal should say why: {err}");
}

/// A command is not an event. The declared set includes commands and envelope
/// payloads too, and an event log holding a `SubmitTurn` would be a category
/// error a looser check would wave through.
#[test]
fn a_declared_command_is_not_a_declared_event() {
    let mut e = engine();
    assert!(e
        .record_event(
            vitna_protocol::type_url::command::SUBMIT_TURN,
            &serde_json::json!({}),
        )
        .is_err());
}

/// The audit names must stay outside the declared namespace, or the guard
/// above would accept them for the wrong reason and a client would decode one
/// as a declared message with every field empty.
#[test]
fn no_audit_name_is_a_declared_name() {
    for name in audit::ALL {
        assert!(
            !vitna_protocol::type_url::is_declared(name),
            "{name} must not be declared"
        );
        assert!(
            name.starts_with(audit::PREFIX),
            "{name} must carry the audit prefix"
        );
        assert!(
            !name.starts_with(vitna_protocol::type_url::PREFIX),
            "{name} must not carry the declared prefix"
        );
    }
}
