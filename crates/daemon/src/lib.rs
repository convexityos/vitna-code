//! Local daemon engine managing session lifecycle, persistence, and IPC dispatch.

pub mod ipc;
pub mod server;

pub use server::{DaemonServer, SessionInfo};

/// The providers whose credentials actually resolve in this process.
///
/// Presence of the variable, not a proof that the key works. The window says
/// "key present" for the same reason: a checked fact, stated as narrowly as it
/// was checked.
pub fn providers_ready() -> Vec<String> {
    let mut ready = Vec::new();
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        ready.push("anthropic".to_string());
    }
    if std::env::var("OPENAI_API_KEY").is_ok() {
        ready.push("openai".to_string());
    }
    ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::fs;
    use std::sync::Mutex;
    use vitna_protocol::api::SubmitTurnRequest;
    use vitna_providers::{
        Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
    };
    use vitna_runner::FakeRunner;
    use vitna_store::EventStore;

    /// A provider that answers from a script.
    ///
    /// `FakeProvider` in `vitna-providers` replays a stream fixture and does
    /// not implement `Provider` at all, so it cannot stand in here. This one
    /// exists to prove the loop does what a model asks, which is the property
    /// no test could reach while the daemon matched keywords instead.
    struct ScriptedProvider {
        name: String,
        replies: Mutex<Vec<ProviderResponse>>,
        /// Every request the loop sent, so a test can assert on what the model
        /// was actually shown.
        seen: Mutex<Vec<ProviderRequest>>,
    }

    impl ScriptedProvider {
        fn new(name: &str, replies: Vec<ProviderResponse>) -> Self {
            Self {
                name: name.to_string(),
                replies: Mutex::new(replies),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    fn reply(content: &str, tool_calls: Vec<ProviderToolCall>) -> ProviderResponse {
        ProviderResponse {
            content: content.to_string(),
            tool_calls,
            prompt_tokens: 11,
            completion_tokens: 7,
            finish_reason: "end_turn".to_string(),
        }
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn parse_sse_event(
            &self,
            _event_type: &str,
            _data: &str,
        ) -> Result<Vec<ProviderStreamEvent>, String> {
            Ok(Vec::new())
        }

        async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
            self.seen.lock().unwrap().push(req.clone());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err("the script ran out of replies".to_string());
            }
            Ok(replies.remove(0))
        }
    }

    fn daemon_in(dir: &std::path::Path) -> DaemonServer {
        let store = EventStore::open_in_memory().expect("open store");
        let runner = FakeRunner::new(dir.join("test.journal")).expect("fake runner");
        DaemonServer::new(
            store,
            std::sync::Arc::new(std::sync::Mutex::new(runner)),
            vitna_receipts::generate_signing_key(),
        )
    }

    #[tokio::test]
    async fn a_session_is_created_and_listed() {
        let dir = std::env::temp_dir().join(format!("vitna_daemon_sess_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let daemon = daemon_in(&dir);

        let session = daemon.create_session(&dir).expect("create session");
        assert_eq!(session.status, "active");

        let list = daemon.list_sessions();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, session.session_id);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_turn_runs_the_tool_the_model_asked_for_and_the_receipt_names_the_real_provider() {
        let dir = std::env::temp_dir().join(format!("vitna_daemon_turn_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let daemon = daemon_in(&dir);
        let session = daemon.create_session(&dir).expect("create session");

        // Round one asks for a file. Round two concludes.
        let provider = ScriptedProvider::new(
            "scripted",
            vec![
                reply(
                    "",
                    vec![ProviderToolCall {
                        id: "call-1".to_string(),
                        name: "write_file".to_string(),
                        arguments: serde_json::json!({
                            "path": "NOTES.md",
                            "content": "written by the model\n"
                        }),
                    }],
                ),
                reply("Wrote NOTES.md.", vec![]),
            ],
        );

        let result = daemon
            .run_turn_with(
                &provider,
                &SubmitTurnRequest {
                    session_id: session.session_id.clone(),
                    prompt: "leave a note".to_string(),
                    provider: None,
                    model_sku: Some("scripted-model-1".to_string()),
                    auto_approve: true,
                    verification_command: None,
                },
            )
            .await
            .expect("the turn runs");

        // The file is on disk because the model asked for it, not because the
        // prompt happened to contain a keyword.
        let written = fs::read_to_string(dir.join("NOTES.md")).expect("the model's file exists");
        assert_eq!(written, "written by the model\n");
        assert!(result.files_modified.iter().any(|p| p.contains("NOTES.md")));

        // The receipt reports what served the turn. It said "fake" before,
        // unconditionally, because that string was a hardcoded default.
        assert_eq!(result.provider, "scripted");
        assert_eq!(result.model_sku, "scripted-model-1");
        assert_eq!(result.completion_state, "completed_with_evidence");
        assert_eq!(result.text, "Wrote NOTES.md.");
        assert_eq!(result.prompt_tokens, Some(22));
        assert_eq!(result.completion_tokens, Some(14));

        // The model was shown the tools, and then the result of its own call.
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "two rounds");
        assert!(
            seen[0].tools.iter().any(|t| t["name"] == "write_file"),
            "the schemas reached the model"
        );
        let last = seen[1].messages.last().expect("a closing message");
        assert_eq!(last.tool_call_id.as_deref(), Some("call-1"));

        let receipt_on_disk =
            fs::read_to_string(&result.receipt_path).expect("the receipt is where it said");
        assert!(receipt_on_disk.contains("scripted"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_turns_prompt_is_read_back_off_the_event_log() {
        let dir = std::env::temp_dir().join(format!("vitna_daemon_turns_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let daemon = daemon_in(&dir);
        let session = daemon.create_session(&dir).expect("create session");

        assert!(daemon.list_turns().expect("list").turns.is_empty(), "no turn yet");

        let provider = ScriptedProvider::new("scripted", vec![reply("Nothing to change.", vec![])]);
        let result = daemon
            .run_turn_with(
                &provider,
                &SubmitTurnRequest {
                    session_id: session.session_id.clone(),
                    prompt: "tidy the readme".to_string(),
                    provider: None,
                    model_sku: Some("scripted-model-1".to_string()),
                    auto_approve: true,
                    verification_command: None,
                },
            )
            .await
            .expect("the turn runs");

        // A TurnStarted the log cannot read back is reported, not dropped.
        {
            let store = daemon.store.lock().unwrap();
            store
                .append_event(&vitna_store::EventRecord::new(
                    "ev-broken-0",
                    "run-broken",
                    0,
                    server::TURN_STARTED,
                    1,
                    b"not json".to_vec(),
                    vitna_store::GENESIS_HASH,
                ))
                .expect("append");
        }

        let list = daemon.list_turns().expect("list");
        assert_eq!(list.turns.len(), 1);
        let turn = &list.turns[0];
        assert_eq!(turn.prompt, "tidy the readme");
        assert_eq!(turn.session_id, session.session_id);
        assert_eq!(turn.run_id, result.run_id, "the prompt names the run its receipt names");
        assert!(turn.started_at_ms > 0);

        assert_eq!(list.unreadable.len(), 1);
        assert_eq!(list.unreadable[0].run_id, "run-broken");
        assert!(list.unreadable[0].reason.contains("not JSON"), "{}", list.unreadable[0].reason);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_turn_without_a_model_is_refused_rather_than_defaulted() {
        let dir = std::env::temp_dir().join(format!("vitna_daemon_nomodel_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let daemon = daemon_in(&dir);
        let session = daemon.create_session(&dir).expect("create session");

        let provider = ScriptedProvider::new("scripted", vec![reply("hi", vec![])]);
        let err = daemon
            .run_turn_with(
                &provider,
                &SubmitTurnRequest {
                    session_id: session.session_id,
                    prompt: "anything".to_string(),
                    provider: None,
                    model_sku: None,
                    auto_approve: true,
                    verification_command: None,
                },
            )
            .await
            .expect_err("a turn with no model chosen is refused");
        assert!(
            err.contains("model"),
            "the reason names the missing choice: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
