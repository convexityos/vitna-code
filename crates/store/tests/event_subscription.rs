//! The store's half of a subscription: exclusive reads, and fan-out on append.
//!
//! These exist because the transport above them filters defensively. A
//! subscription tracks the last sequence it actually sent and skips anything
//! at or below it, which is correct, and which also means an off-by-one in
//! [`EventStore::events_after`] cannot be seen from there: the two mistakes
//! cancel. Caught by mutation testing the transport, where making
//! `events_after` inclusive left every wire test green.
//!
//! So the boundary is tested where it is decided, not where it is consumed.

use vitna_store::{EventRecord, EventStore, GENESIS_HASH};

const RUN: &str = "run-events-after";

fn store_with(count: u64) -> EventStore {
    let store = EventStore::open_in_memory().expect("open store");
    let mut prev = GENESIS_HASH.to_string();
    for i in 0..count {
        let sequence = vitna_protocol::FIRST_EVENT_SEQUENCE + i;
        let event = EventRecord::new(
            format!("ev-{sequence}"),
            RUN,
            sequence,
            "type.vitna.ai/vitna.protocol.v1.Diagnostic",
            1_000 + sequence,
            Vec::new(),
            &prev,
        );
        prev = event.event_hash.clone();
        store.append_event(&event).expect("append");
    }
    store
}

fn sequences(events: &[EventRecord]) -> Vec<u64> {
    events.iter().map(|e| e.sequence).collect()
}

/// `resume_after_sequence` names the last event the client HOLDS. Returning it
/// again would be a duplicate; skipping the one after it would be a loss. The
/// two are one character apart in the implementation.
#[test]
fn events_after_is_exclusive_of_the_sequence_named() {
    let store = store_with(4);

    assert_eq!(
        sequences(&store.events_after(RUN, 2).expect("read")),
        vec![3, 4],
        "after 2 means 3 onward, never 2 again"
    );
    assert_eq!(
        sequences(&store.events_after(RUN, 4).expect("read")),
        Vec::<u64>::new(),
        "a client holding everything gets nothing, not the last event again"
    );
}

/// The case the whole numbering convention exists for.
#[test]
fn events_after_zero_is_every_event() {
    let store = store_with(3);

    assert_eq!(
        sequences(&store.events_after(RUN, 0).expect("read")),
        vec![1, 2, 3],
        "proto3 cannot distinguish an unset resume_after_sequence from 0, so 0 \
         must yield the whole run including its first event"
    );
}

/// Distinct from `get_events`, which is inclusive and is what replay wants.
/// Confusing the two is invisible at a glance and produces either a duplicate
/// or a hole.
#[test]
fn events_after_and_get_events_differ_by_exactly_one_event() {
    let store = store_with(4);

    assert_eq!(
        sequences(&store.get_events(RUN, 2).expect("read")),
        vec![2, 3, 4]
    );
    assert_eq!(
        sequences(&store.events_after(RUN, 2).expect("read")),
        vec![3, 4]
    );
}

#[test]
fn saturating_at_the_top_of_the_range_does_not_wrap() {
    let store = store_with(1);
    assert!(
        store.events_after(RUN, u64::MAX).expect("read").is_empty(),
        "u64::MAX + 1 must not wrap round to 0 and return the whole run"
    );
}

#[tokio::test]
async fn a_subscriber_receives_what_is_appended_after_it_subscribed() {
    let store = EventStore::open_in_memory().expect("open store");
    let mut rx = store.subscribe();

    let event = EventRecord::new(
        "ev-1",
        RUN,
        vitna_protocol::FIRST_EVENT_SEQUENCE,
        "type.vitna.ai/vitna.protocol.v1.Diagnostic",
        1_000,
        Vec::new(),
        GENESIS_HASH,
    );
    store.append_event(&event).expect("append");

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("an appended event should reach a subscriber")
        .expect("channel open");

    assert_eq!(received.sequence, vitna_protocol::FIRST_EVENT_SEQUENCE);
    assert_eq!(received.event_hash, event.event_hash);
}

/// Fan-out is an optimization over reading the store, so nobody listening is
/// an ordinary state and must not turn an append into a failure.
#[test]
fn an_append_with_no_subscribers_still_succeeds() {
    let store = EventStore::open_in_memory().expect("open store");
    let event = EventRecord::new(
        "ev-1",
        RUN,
        vitna_protocol::FIRST_EVENT_SEQUENCE,
        "type.vitna.ai/vitna.protocol.v1.Diagnostic",
        1_000,
        Vec::new(),
        GENESIS_HASH,
    );

    store
        .append_event(&event)
        .expect("an append with no listeners must succeed");
    assert_eq!(
        sequences(&store.events_after(RUN, 0).expect("read")),
        vec![1]
    );
}

/// A rejected append must not reach a subscriber. The stream would then be
/// claiming an event the durable record does not hold, which is the one thing
/// a tamper-evident log cannot do.
#[tokio::test]
async fn an_append_that_fails_integrity_is_not_published() {
    let store = EventStore::open_in_memory().expect("open store");
    let mut rx = store.subscribe();

    let mut tampered = EventRecord::new(
        "ev-1",
        RUN,
        vitna_protocol::FIRST_EVENT_SEQUENCE,
        "type.vitna.ai/vitna.protocol.v1.Diagnostic",
        1_000,
        Vec::new(),
        GENESIS_HASH,
    );
    tampered.event_hash = "0".repeat(64);

    assert!(
        store.append_event(&tampered).is_err(),
        "integrity must be checked"
    );
    assert!(
        rx.try_recv().is_err(),
        "a refused append must not appear on the live stream"
    );
}
