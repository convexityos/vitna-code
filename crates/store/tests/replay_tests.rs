//! Integration tests for EventStore replay and crash recovery fixtures.

use std::fs;
use std::path::PathBuf;
use vitna_store::{EventRecord, EventStore, ReplayedRunState};

fn load_fixture(name: &str) -> Vec<EventRecord> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // to crates/
    path.pop(); // to repo root
    path.push("fixtures");
    path.push("crash-recovery");
    path.push(name);

    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture at {:?}: {}", path, e));
    serde_json::from_str(&content).expect("invalid fixture json")
}

#[test]
fn test_fixture_clean_run_replay() {
    let store = EventStore::open_in_memory().expect("open store");
    let events = load_fixture("clean_run.json");

    for event in &events {
        store.append_event(event).expect("append event");
    }

    let summary = store.replay_run("run-recovery-clean").expect("replay run");
    assert_eq!(summary.total_events, 5);
    assert_eq!(summary.last_sequence, 4);
    assert_eq!(summary.state, ReplayedRunState::Completed);
}

#[test]
fn test_fixture_crash_mid_tool_leaves_open_effect() {
    let store = EventStore::open_in_memory().expect("open store");
    let events = load_fixture("crash_mid_tool.json");

    for event in &events {
        store.append_event(event).expect("append event");
    }

    let summary = store.replay_run("run-recovery-crash-mid").expect("replay run");
    assert_eq!(summary.total_events, 3);
    assert_eq!(summary.last_sequence, 2);
    // Unfinished tool invocation leaves run in RunningTool state awaiting reconciliation
    assert_eq!(summary.state, ReplayedRunState::RunningTool);
}

#[test]
fn test_fixture_crash_post_finish_unack() {
    let store = EventStore::open_in_memory().expect("open store");
    let events = load_fixture("crash_post_finish_unack.json");

    for event in &events {
        store.append_event(event).expect("append event");
    }

    let summary = store.replay_run("run-recovery-crash-post").expect("replay run");
    assert_eq!(summary.total_events, 4);
    assert_eq!(summary.last_sequence, 3);
    // Tool finished, awaiting turn completion event
    assert_eq!(summary.state, ReplayedRunState::RunningModel);
}
