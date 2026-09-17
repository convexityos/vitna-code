//! Vitna Event Store, SQLite persistence layer, and event replay state machine.

use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub type_url: String,
    pub timestamp_ms: u64,
    pub encrypted_payload: Vec<u8>,
    pub prev_event_hash: String,
    pub event_hash: String,
}

impl EventRecord {
    pub fn new(
        event_id: impl Into<String>,
        run_id: impl Into<String>,
        sequence: u64,
        type_url: impl Into<String>,
        timestamp_ms: u64,
        encrypted_payload: Vec<u8>,
        prev_event_hash: impl Into<String>,
    ) -> Self {
        let event_id = event_id.into();
        let run_id = run_id.into();
        let type_url = type_url.into();
        let prev_event_hash = prev_event_hash.into();

        let event_hash = Self::calculate_hash(
            &prev_event_hash,
            &run_id,
            sequence,
            &type_url,
            timestamp_ms,
            &encrypted_payload,
        );

        Self {
            event_id,
            run_id,
            sequence,
            type_url,
            timestamp_ms,
            encrypted_payload,
            prev_event_hash,
            event_hash,
        }
    }

    pub fn calculate_hash(
        prev_hash: &str,
        run_id: &str,
        sequence: u64,
        type_url: &str,
        timestamp_ms: u64,
        payload: &[u8],
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_bytes());
        hasher.update(b":");
        hasher.update(run_id.as_bytes());
        hasher.update(b":");
        hasher.update(sequence.to_string().as_bytes());
        hasher.update(b":");
        hasher.update(type_url.as_bytes());
        hasher.update(b":");
        hasher.update(timestamp_ms.to_string().as_bytes());
        hasher.update(b":");

        let mut payload_hasher = Sha256::new();
        payload_hasher.update(payload);
        let payload_hash = hex::encode(payload_hasher.finalize());
        hasher.update(payload_hash.as_bytes());

        hex::encode(hasher.finalize())
    }

    pub fn verify_integrity(&self) -> bool {
        let expected = Self::calculate_hash(
            &self.prev_event_hash,
            &self.run_id,
            self.sequence,
            &self.type_url,
            self.timestamp_ms,
            &self.encrypted_payload,
        );
        self.event_hash == expected
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayedRunState {
    Queued,
    RunningModel,
    WaitingForApproval,
    RunningTool,
    WaitingForChild,
    WaitingForRetry,
    Paused,
    NeedsReconciliation,
    Completed,
    Failed,
    Cancelled,
    Orphaned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub last_sequence: u64,
    pub state: ReplayedRunState,
    pub last_event_hash: String,
    pub total_events: usize,
}

pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn open<P: AsRef<Path>>(path: P) -> SqliteResult<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.configure_pragmas()?;
        store.apply_migrations()?;
        Ok(store)
    }

    pub fn open_in_memory() -> SqliteResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.configure_pragmas()?;
        store.apply_migrations()?;
        Ok(store)
    }

    fn configure_pragmas(&self) -> SqliteResult<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
    }

    pub fn apply_migrations(&self) -> SqliteResult<()> {
        let sql = include_str!("../migrations/0001_initial_schema.sql");
        self.conn.execute_batch(sql)
    }

    pub fn append_event(&self, event: &EventRecord) -> SqliteResult<()> {
        if !event.verify_integrity() {
            return Err(rusqlite::Error::InvalidQuery);
        }

        self.conn.execute(
            "INSERT INTO events (
                event_id, run_id, sequence, type_url, timestamp_ms,
                encrypted_payload, prev_event_hash, event_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.event_id,
                event.run_id,
                event.sequence,
                event.type_url,
                event.timestamp_ms,
                event.encrypted_payload,
                event.prev_event_hash,
                event.event_hash,
            ],
        )?;

        Ok(())
    }

    pub fn get_events(&self, run_id: &str, since_sequence: u64) -> SqliteResult<Vec<EventRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, run_id, sequence, type_url, timestamp_ms,
                    encrypted_payload, prev_event_hash, event_hash
             FROM events
             WHERE run_id = ?1 AND sequence >= ?2
             ORDER BY sequence ASC",
        )?;

        let rows = stmt.query_map(params![run_id, since_sequence], |row| {
            Ok(EventRecord {
                event_id: row.get(0)?,
                run_id: row.get(1)?,
                sequence: row.get(2)?,
                type_url: row.get(3)?,
                timestamp_ms: row.get(4)?,
                encrypted_payload: row.get(5)?,
                prev_event_hash: row.get(6)?,
                event_hash: row.get(7)?,
            })
        })?;

        let mut events = Vec::new();
        for r in rows {
            events.push(r?);
        }
        Ok(events)
    }

    /// Replays events for a given run from sequence zero, validating cryptographic integrity
    /// and reconstructing the current state.
    pub fn replay_run(&self, run_id: &str) -> SqliteResult<RunSummary> {
        let events = self.get_events(run_id, 0)?;

        let mut expected_prev_hash = GENESIS_HASH.to_string();
        let mut current_state = ReplayedRunState::Queued;
        let mut last_seq = 0;
        let mut last_hash = GENESIS_HASH.to_string();

        for event in &events {
            // Verify hash chain consistency
            if event.prev_event_hash != expected_prev_hash {
                return Ok(RunSummary {
                    run_id: run_id.to_string(),
                    last_sequence: last_seq,
                    state: ReplayedRunState::NeedsReconciliation,
                    last_event_hash: last_hash,
                    total_events: events.len(),
                });
            }

            if !event.verify_integrity() {
                return Ok(RunSummary {
                    run_id: run_id.to_string(),
                    last_sequence: last_seq,
                    state: ReplayedRunState::NeedsReconciliation,
                    last_event_hash: last_hash,
                    total_events: events.len(),
                });
            }

            // Simple transition mapping based on type_url
            if event.type_url.contains("ToolProposed") || event.type_url.contains("ApprovalRequested") {
                current_state = ReplayedRunState::WaitingForApproval;
            } else if event.type_url.contains("ToolStarted") {
                current_state = ReplayedRunState::RunningTool;
            } else if event.type_url.contains("ToolFinished") {
                current_state = ReplayedRunState::RunningModel;
            } else if event.type_url.contains("RunStateChanged") {
                // If the event payload indicates completed/failed, map state
                if event.encrypted_payload.windows(9).any(|w| w == b"completed") {
                    current_state = ReplayedRunState::Completed;
                } else if event.encrypted_payload.windows(6).any(|w| w == b"failed") {
                    current_state = ReplayedRunState::Failed;
                } else if event.encrypted_payload.windows(20).any(|w| w == b"needs_reconciliation") {
                    current_state = ReplayedRunState::NeedsReconciliation;
                }
            }

            expected_prev_hash = event.event_hash.clone();
            last_hash = event.event_hash.clone();
            last_seq = event.sequence;
        }

        Ok(RunSummary {
            run_id: run_id.to_string(),
            last_sequence: last_seq,
            state: current_state,
            last_event_hash: last_hash,
            total_events: events.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_hash_chain() {
        let store = EventStore::open_in_memory().expect("in-memory db open failed");

        let e1 = EventRecord::new(
            "evt-1",
            "run-001",
            0,
            "type.vitna.ai/vitna.protocol.v1.RunStateChanged",
            1000,
            b"running_model".to_vec(),
            GENESIS_HASH,
        );
        store.append_event(&e1).expect("append e1 failed");

        let e2 = EventRecord::new(
            "evt-2",
            "run-001",
            1,
            "type.vitna.ai/vitna.protocol.v1.ToolProposed",
            1100,
            b"read_file".to_vec(),
            &e1.event_hash,
        );
        store.append_event(&e2).expect("append e2 failed");

        let summary = store.replay_run("run-001").expect("replay must succeed");
        assert_eq!(summary.total_events, 2);
        assert_eq!(summary.last_sequence, 1);
        assert_eq!(summary.last_event_hash, e2.event_hash);
        assert_eq!(summary.state, ReplayedRunState::WaitingForApproval);
    }

    #[test]
    fn test_corrupt_hash_chain_forces_reconciliation() {
        let store = EventStore::open_in_memory().expect("in-memory db open failed");

        let e1 = EventRecord::new(
            "evt-1",
            "run-002",
            0,
            "type.vitna.ai/vitna.protocol.v1.RunStateChanged",
            1000,
            b"running_model".to_vec(),
            GENESIS_HASH,
        );
        store.append_event(&e1).expect("append e1 failed");

        // Malicious or corrupted event pointing to wrong prev_hash
        let mut e2 = EventRecord::new(
            "evt-2",
            "run-002",
            1,
            "type.vitna.ai/vitna.protocol.v1.ToolProposed",
            1100,
            b"read_file".to_vec(),
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        );
        // Bypass internal verify_integrity on insert to simulate raw DB tamper
        let _ = store.conn.execute(
            "INSERT INTO events VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e2.event_id,
                e2.run_id,
                e2.sequence,
                e2.type_url,
                e2.timestamp_ms,
                e2.encrypted_payload,
                e2.prev_event_hash,
                e2.event_hash,
            ],
        );

        let summary = store.replay_run("run-002").expect("replay must complete");
        assert_eq!(summary.state, ReplayedRunState::NeedsReconciliation);
    }
}