use std::path::PathBuf;
use vitna_providers::{
    AnthropicProvider, FakeProvider, OpenAIProvider, ProviderCredentials, ProviderStreamEvent,
};

fn get_fixtures_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("../../fixtures/providers")
}

#[test]
fn test_fake_provider_streaming_turn_replay() {
    let fixture_path = get_fixtures_dir().join("streaming_turn.json");
    assert!(fixture_path.exists(), "streaming_turn.json fixture must exist");

    let provider = FakeProvider::from_fixture_file(&fixture_path).expect("load fixture");
    let stream = provider.replay_stream();

    let deltas: Vec<&str> = stream.iter().map(|e| e.delta_text.as_str()).collect();
    assert_eq!(
        deltas,
        vec![
            "I have ",
            "analyzed the ",
            "repository structure. ",
            "Here is the ",
            "evidence-backed plan.",
        ]
    );

    // streaming_turn.json declares no finish reason, so the replay states none.
    assert!(stream.iter().all(|e| e.finish_reason.is_none()));
}

#[test]
fn test_anthropic_sse_stream_sequence() {
    let creds = ProviderCredentials {
        provider: "anthropic".to_string(),
        api_key: "sk-ant-test".to_string(),
        base_url: None,
    };
    let provider = AnthropicProvider::new(creds);

    let lines = vec![
        ("content_block_start", r#"{"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_42", "name": "write_file"}}"#),
        ("content_block_delta", r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"path\": \"src/lib.rs\","}}"#),
        ("content_block_delta", r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": " \"content\": \"pub fn run() {}\"}"}}"#),
        ("message_delta", r#"{"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 48}}"#),
        ("message_stop", r#"{"type": "message_stop"}"#),
    ];

    let mut accumulated_args = String::new();
    let mut tool_name = None;

    for (event_type, data) in lines {
        let events = provider.parse_sse_event_line(event_type, data).expect("parse sse");
        for ev in events {
            match ev {
                ProviderStreamEvent::ToolCallDelta { name, arguments_delta, .. } => {
                    if let Some(n) = name {
                        tool_name = Some(n);
                    }
                    accumulated_args.push_str(&arguments_delta);
                }
                _ => {}
            }
        }
    }

    assert_eq!(tool_name, Some("write_file".to_string()));
    assert!(accumulated_args.contains("src/lib.rs"));
    assert!(accumulated_args.contains("pub fn run() {}"));

    // Verify valid JSON reconstruction
    let parsed: serde_json::Value = serde_json::from_str(&accumulated_args).expect("valid JSON");
    assert_eq!(parsed["path"], "src/lib.rs");
}

#[test]
fn test_openai_sse_stream_sequence() {
    let creds = ProviderCredentials {
        provider: "openai".to_string(),
        api_key: "sk-proj-test".to_string(),
        base_url: None,
    };
    let provider = OpenAIProvider::new(creds);

    let chunks = vec![
        r#"{"choices": [{"delta": {"content": "I will inspect "}}]}"#,
        r#"{"choices": [{"delta": {"content": "the workspace directory."}}]}"#,
        r#"{"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 15, "completion_tokens": 10}}"#,
        "[DONE]",
    ];

    let mut full_text = String::new();
    let mut finished = false;

    for chunk in chunks {
        let events = provider.parse_sse_event_line(chunk).expect("parse sse");
        for ev in events {
            match ev {
                ProviderStreamEvent::TextDelta(t) => full_text.push_str(&t),
                ProviderStreamEvent::Finish(_) => finished = true,
                _ => {}
            }
        }
    }

    assert_eq!(full_text, "I will inspect the workspace directory.");
    assert!(finished);
}
