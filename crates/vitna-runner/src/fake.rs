//! Fake execution runner with synthetic fault injection for reliability testing.

use crate::journal::{ActionJournal, JournalEntry};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultInjectionMode {
    None,
    CrashBeforeStarted,
    CrashMidExecution,
    CrashPostFinishUnack,
    SimulateTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerFault {
    CrashedBeforeStarted,
    CrashedMidExecution,
    CrashedPostFinishUnack,
    TimeoutExceeded,
    IoError(String),
}

pub struct FakeRunner {
    pub journal: ActionJournal,
}

impl FakeRunner {
    pub fn new<P: AsRef<Path>>(journal_path: P) -> io::Result<Self> {
        let journal = ActionJournal::open(journal_path)?;
        Ok(Self { journal })
    }

    /// Executes an action under the 5-step action protocol with optional fault injection.
    pub fn execute_action(
        &mut self,
        action_id: &str,
        argument_digest: &str,
        fault: FaultInjectionMode,
        timestamp_ms: u64,
    ) -> Result<JournalEntry, RunnerFault> {
        // Step 2: Record prepared
        self.journal
            .record_prepared(action_id, argument_digest, timestamp_ms)
            .map_err(|e| RunnerFault::IoError(e.to_string()))?;

        if fault == FaultInjectionMode::CrashBeforeStarted {
            return Err(RunnerFault::CrashedBeforeStarted);
        }

        // Step 3: Record started
        self.journal
            .record_started(action_id, timestamp_ms + 10)
            .map_err(|e| RunnerFault::IoError(e.to_string()))?;

        if fault == FaultInjectionMode::CrashMidExecution {
            return Err(RunnerFault::CrashedMidExecution);
        }

        if fault == FaultInjectionMode::SimulateTimeout {
            return Err(RunnerFault::TimeoutExceeded);
        }

        // Step 4: Record finished
        self.journal
            .record_finished(
                action_id,
                0,
                "fake-stdout-hash".to_string(),
                "fake-stderr-hash".to_string(),
                timestamp_ms + 100,
            )
            .map_err(|e| RunnerFault::IoError(e.to_string()))?;

        if fault == FaultInjectionMode::CrashPostFinishUnack {
            return Err(RunnerFault::CrashedPostFinishUnack);
        }

        // Step 5: Record acknowledged
        self.journal
            .record_acknowledged(action_id)
            .map_err(|e| RunnerFault::IoError(e.to_string()))?;

        self.journal
            .query_action(action_id)
            .cloned()
            .ok_or_else(|| RunnerFault::IoError("missing entry".to_string()))
    }
}

#[async_trait]
impl crate::Runner for std::sync::Mutex<FakeRunner> {
    async fn run_command(
        &self,
        command: &str,
        _working_dir: &Path,
        _timeout_ms: u64,
    ) -> Result<crate::ExecutionOutput, String> {
        let action_id = format!(
            "fake-act-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let arg_digest = hex::encode(Sha256::digest(command.as_bytes()));
        let mut runner = self.lock().map_err(|e| e.to_string())?;
        let entry = runner
            .execute_action(&action_id, &arg_digest, FaultInjectionMode::None, 1000)
            .map_err(|e| format!("{:?}", e))?;

        let exit_code = entry.exit_code.unwrap_or(0);
        let stdout = format!("Fake output for: {}", command);
        let stderr = String::new();

        Ok(crate::ExecutionOutput {
            statement_digest: crate::compute_statement_digest(
                &action_id, command, exit_code, &stdout, &stderr,
            ),
            exit_code,
            stdout,
            stderr,
            duration_ms: 5,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::journal::ActionState;
    use super::*;

    #[test]
    fn test_fake_runner_clean_execution() {
        let temp_file = std::env::temp_dir().join(format!("fake_runner_clean_{}.journal", std::process::id()));
        let mut runner = FakeRunner::new(&temp_file).expect("open fake runner");

        let entry = runner
            .execute_action("act-clean", "arg-1", FaultInjectionMode::None, 1000)
            .expect("execution must succeed");

        assert_eq!(entry.state, ActionState::Acknowledged);
        assert_eq!(entry.exit_code, Some(0));

        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_fake_runner_crash_before_started() {
        let temp_file = std::env::temp_dir().join(format!("fake_runner_crash1_{}.journal", std::process::id()));
        let mut runner = FakeRunner::new(&temp_file).expect("open fake runner");

        let res = runner.execute_action("act-crash1", "arg-1", FaultInjectionMode::CrashBeforeStarted, 1000);
        assert_eq!(res, Err(RunnerFault::CrashedBeforeStarted));

        let entry = runner.journal.query_action("act-crash1").expect("entry exists");
        assert_eq!(entry.state, ActionState::Prepared);

        // Crash recovery flags it
        let reconciled = runner.journal.reconcile_crashed_actions().expect("reconcile");
        assert_eq!(reconciled, vec!["act-crash1".to_string()]);

        let recovered = runner.journal.query_action("act-crash1").expect("entry exists");
        assert_eq!(recovered.state, ActionState::NeedsReconciliation);

        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_fake_runner_crash_mid_execution() {
        let temp_file = std::env::temp_dir().join(format!("fake_runner_crash2_{}.journal", std::process::id()));
        let mut runner = FakeRunner::new(&temp_file).expect("open fake runner");

        let res = runner.execute_action("act-crash2", "arg-1", FaultInjectionMode::CrashMidExecution, 1000);
        assert_eq!(res, Err(RunnerFault::CrashedMidExecution));

        let entry = runner.journal.query_action("act-crash2").expect("entry exists");
        assert_eq!(entry.state, ActionState::Started);

        // Crash recovery flags it as needs_reconciliation
        let reconciled = runner.journal.reconcile_crashed_actions().expect("reconcile");
        assert_eq!(reconciled, vec!["act-crash2".to_string()]);

        let recovered = runner.journal.query_action("act-crash2").expect("entry exists");
        assert_eq!(recovered.state, ActionState::NeedsReconciliation);

        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_fake_runner_crash_post_finish_unack() {
        let temp_file = std::env::temp_dir().join(format!("fake_runner_crash3_{}.journal", std::process::id()));
        let mut runner = FakeRunner::new(&temp_file).expect("open fake runner");

        let res = runner.execute_action("act-crash3", "arg-1", FaultInjectionMode::CrashPostFinishUnack, 1000);
        assert_eq!(res, Err(RunnerFault::CrashedPostFinishUnack));

        // Result is retained in finished state
        let entry = runner.journal.query_action("act-crash3").expect("entry exists");
        assert_eq!(entry.state, ActionState::Finished);
        assert_eq!(entry.exit_code, Some(0));

        // Daemon acknowledges upon reconnect without re-running
        runner.journal.record_acknowledged("act-crash3").expect("acknowledge");
        let acknowledged = runner.journal.query_action("act-crash3").expect("entry exists");
        assert_eq!(acknowledged.state, ActionState::Acknowledged);

        let _ = std::fs::remove_file(temp_file);
    }
}
