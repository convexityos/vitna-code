//! Local daemon engine managing session lifecycle, persistence, and IPC dispatch.

pub mod ipc;
pub mod launch;
pub mod server;

pub use server::{DaemonServer, SessionInfo};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use vitna_runner::FakeRunner;
    use vitna_store::EventStore;

    #[tokio::test]
    async fn test_daemon_server_session_and_run() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_daemon_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let store = EventStore::open_in_memory().expect("open store");
        let journal_path = temp_dir.join("daemon_test.journal");
        let fake_runner = FakeRunner::new(&journal_path).expect("fake runner");
        let runner = std::sync::Arc::new(std::sync::Mutex::new(fake_runner));
        let signing_key = vitna_receipts::generate_signing_key();

        let daemon = DaemonServer::new(store, runner, signing_key);

        // Create session
        let session = daemon
            .create_session(&temp_dir)
            .expect("create session succeeds");
        assert_eq!(session.status, "active");

        // List sessions
        let list = daemon.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, session.session_id);

        // Run a task
        let receipt = daemon
            .run_task(
                &session.session_id,
                "Please add a new configuration file",
                true,
                Some("cargo --version".to_string()),
                // This test drives the fake runner, which starts no process and
                // so needs no sandbox. Left false so the test keeps asserting
                // the default posture rather than the permissive one.
                false,
            )
            .await
            .expect("run task succeeds");

        assert_eq!(receipt.session_id, session.session_id);
        assert_eq!(receipt.completion_state, "completed_with_evidence");
        assert!(!receipt.device_signature.is_empty());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}