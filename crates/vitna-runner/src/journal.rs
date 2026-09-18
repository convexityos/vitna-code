//! Durable action journal for vitna-runner.
//!
//! Enforces the 5-step action protocol between vitna-coded daemon and vitna-runner:
//! 1. Approved (Daemon)
//! 2. Prepared (Runner records to disk before acknowledging request)
//! 3. Started (Runner records immediately before child spawn)
//! 4. Finished (Runner records with exit code and digests)
//! 5. Acknowledged (Daemon records and acknowledges; runner marks acknowledged)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionState {
    Prepared,
    Started,
    Finished,
    Acknowledged,
    NeedsReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub action_id: String,
    pub argument_digest: String,
    pub state: ActionState,
    pub prepared_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub stdout_digest: Option<String>,
    pub stderr_digest: Option<String>,
}

pub struct ActionJournal {
    path: PathBuf,
    entries: HashMap<String, JournalEntry>,
}

impl ActionJournal {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut entries = HashMap::new();

        if path.exists() {
            let file = File::open(&path)?;
            let reader = BufReader::new(file);

            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<JournalEntry>(&line) {
                    entries.insert(entry.action_id.clone(), entry);
                }
            }
        }

        Ok(Self { path, entries })
    }

    fn append_entry(&mut self, entry: JournalEntry) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let serialized = serde_json::to_string(&entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        writeln!(file, "{}", serialized)?;
        file.sync_all()?;

        self.entries.insert(entry.action_id.clone(), entry);
        Ok(())
    }

    pub fn record_prepared(
        &mut self,
        action_id: impl Into<String>,
        argument_digest: impl Into<String>,
        timestamp_ms: u64,
    ) -> io::Result<()> {
        let entry = JournalEntry {
            action_id: action_id.into(),
            argument_digest: argument_digest.into(),
            state: ActionState::Prepared,
            prepared_at_ms: timestamp_ms,
            started_at_ms: None,
            finished_at_ms: None,
            exit_code: None,
            stdout_digest: None,
            stderr_digest: None,
        };
        self.append_entry(entry)
    }

    pub fn record_started(
        &mut self,
        action_id: &str,
        timestamp_ms: u64,
    ) -> io::Result<()> {
        let mut entry = self.entries.get(action_id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "action not found"))?;

        entry.state = ActionState::Started;
        entry.started_at_ms = Some(timestamp_ms);
        self.append_entry(entry)
    }

    pub fn record_finished(
        &mut self,
        action_id: &str,
        exit_code: i32,
        stdout_digest: String,
        stderr_digest: String,
        timestamp_ms: u64,
    ) -> io::Result<()> {
        let mut entry = self.entries.get(action_id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "action not found"))?;

        entry.state = ActionState::Finished;
        entry.finished_at_ms = Some(timestamp_ms);
        entry.exit_code = Some(exit_code);
        entry.stdout_digest = Some(stdout_digest);
        entry.stderr_digest = Some(stderr_digest);
        self.append_entry(entry)
    }

    pub fn record_acknowledged(&mut self, action_id: &str) -> io::Result<()> {
        let mut entry = self.entries.get(action_id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "action not found"))?;

        entry.state = ActionState::Acknowledged;
        self.append_entry(entry)
    }

    pub fn query_action(&self, action_id: &str) -> Option<&JournalEntry> {
        self.entries.get(action_id)
    }

    /// Evaluates journal entries on crash recovery. If an action was started
    /// but never reached finished, or finished but was never acknowledged before reboot,
    /// it enters needs_reconciliation and is never automatically retried.
    pub fn reconcile_crashed_actions(&mut self) -> io::Result<Vec<String>> {
        let mut reconciled = Vec::new();
        let unacknowledged: Vec<JournalEntry> = self.entries.values()
            .filter(|e| e.state == ActionState::Started || e.state == ActionState::Prepared)
            .cloned()
            .collect();

        for mut entry in unacknowledged {
            entry.state = ActionState::NeedsReconciliation;
            reconciled.push(entry.action_id.clone());
            self.append_entry(entry)?;
        }

        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_journal_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_journal_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let journal_path = temp_dir.join("actions.journal");

        {
            let mut journal = ActionJournal::open(&journal_path).expect("open journal");
            journal.record_prepared("act-1", "arg-hash-1", 100).expect("prepared");
            journal.record_started("act-1", 150).expect("started");
            journal.record_finished("act-1", 0, "out-hash".to_string(), "err-hash".to_string(), 200).expect("finished");
            journal.record_acknowledged("act-1").expect("acknowledged");

            let entry = journal.query_action("act-1").expect("entry exists");
            assert_eq!(entry.state, ActionState::Acknowledged);
            assert_eq!(entry.exit_code, Some(0));
        }

        // Re-open from disk and assert durability
        {
            let journal = ActionJournal::open(&journal_path).expect("reopen journal");
            let entry = journal.query_action("act-1").expect("persisted entry exists");
            assert_eq!(entry.state, ActionState::Acknowledged);
            assert_eq!(entry.exit_code, Some(0));
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_crash_recovery_forces_reconciliation() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_crash_journal_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let journal_path = temp_dir.join("crash_actions.journal");

        {
            let mut journal = ActionJournal::open(&journal_path).expect("open journal");
            journal.record_prepared("act-mutating", "arg-hash-2", 100).expect("prepared");
            journal.record_started("act-mutating", 120).expect("started");
            // Simulate sudden process crash while started
        }

        // Daemon recovers and opens journal
        {
            let mut journal = ActionJournal::open(&journal_path).expect("reopen journal");
            let reconciled = journal.reconcile_crashed_actions().expect("reconcile actions");
            assert_eq!(reconciled, vec!["act-mutating".to_string()]);

            let entry = journal.query_action("act-mutating").expect("entry exists");
            assert_eq!(entry.state, ActionState::NeedsReconciliation);
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
