//! A tool-using turn, as each adapter writes it and reads it back.
//!
//! Nothing called `complete()` until the daemon's agent loop, so these shapes
//! were never exercised by a second round: a model that asked for a tool, was
//! answered, and asked again. Each API rejects a request whose tool calls and
//! results do not pair up, and each pairs them differently.

use serde_json::json;
use vitna_providers::openai::parse_completion;
use vitna_providers::{
    AnthropicProvider, OpenAIProvider, ProviderCredentials, ProviderMessage, ProviderRequest,
    ProviderToolCall,
};

fn creds(provider: &str) -> ProviderCredentials {
    ProviderCredentials {
        provider: provider.to_string(),
        api_key: "unused".to_string(),
        base_url: None,
    }
}

fn second_round() -> ProviderRequest {
    ProviderRequest {
        model: "m".into(),
        system_prompt: Some("sys".into()),
        messages: vec![
            ProviderMessage::user("look around"),
            ProviderMessage::assistant(
                "",
                vec![
                    ProviderToolCall {
                        id: "c1".into(),
                        name: "list_dir".into(),
                        arguments: json!({ "path": "." }),
                    },
                    ProviderToolCall {
                        id: "c2".into(),
                        name: "read_file".into(),
                        arguments: json!("{not json"),
                    },
                ],
            ),
            ProviderMessage::tool_result("c1", "a.txt"),
            ProviderMessage::tool_result(
                "c2",
                "read_file could not run: arguments are not an object",
            ),
        ],
        tools: vec![],
        temperature: None,
        max_tokens: None,
    }
}

#[test]
fn openai_pairs_each_result_to_its_call_by_id_and_sends_arguments_as_a_string() {
    let payload = OpenAIProvider::new(creds("openai")).format_request_payload(&second_round());
    let messages = payload["messages"].as_array().expect("messages");
    assert_eq!(
        messages.len(),
        5,
        "system, user, assistant, two tool results: {messages:#?}"
    );

    let assistant = &messages[2];
    assert_eq!(assistant["role"], "assistant");
    assert!(
        assistant["content"].is_null(),
        "an assistant turn with no text sends null content"
    );
    let calls = assistant["tool_calls"].as_array().expect("tool_calls");
    assert_eq!(calls[0]["function"]["arguments"], json!("{\"path\":\".\"}"));
    // Raw text the model wrote goes back exactly as it came, not re-quoted.
    assert_eq!(calls[1]["function"]["arguments"], json!("{not json"));

    assert_eq!(
        messages[3],
        json!({ "role": "tool", "tool_call_id": "c1", "content": "a.txt" })
    );
    assert_eq!(messages[4]["tool_call_id"], "c2");
}

#[test]
fn anthropic_folds_every_result_for_one_assistant_turn_into_one_user_message() {
    let payload =
        AnthropicProvider::new(creds("anthropic")).format_request_payload(&second_round());
    let messages = payload["messages"].as_array().expect("messages");
    // user, assistant, ONE user carrying both results. Two consecutive user
    // turns is a request the API refuses.
    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert_eq!(messages[1]["role"], "assistant");
    let blocks = messages[1]["content"].as_array().expect("tool_use blocks");
    assert_eq!(blocks.len(), 2, "no text block for empty text: {blocks:#?}");
    assert_eq!(blocks[0]["type"], "tool_use");
    assert_eq!(blocks[0]["input"], json!({ "path": "." }));

    assert_eq!(messages[2]["role"], "user");
    let results = messages[2]["content"]
        .as_array()
        .expect("tool_result blocks");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_use_id"], "c1");
    assert_eq!(results[1]["tool_use_id"], "c2");
}

#[test]
fn a_completion_names_the_model_that_served_it_and_keeps_unparsed_arguments_as_text() {
    let body = json!({
        "model": "deepseek/deepseek-v4.1-flash",
        "choices": [{
            "message": {
                "content": "checking",
                "tool_calls": [
                    { "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a.txt\"}" } },
                    { "id": "c2", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\": " } }
                ]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 3 }
    });
    let response = parse_completion(&body);
    assert_eq!(
        response.model.as_deref(),
        Some("deepseek/deepseek-v4.1-flash")
    );
    assert_eq!(response.finish_reason, "tool_calls");
    assert_eq!(response.tool_calls[0].arguments, json!({ "path": "a.txt" }));
    // Was `{}`, which ran the tool on arguments nobody sent.
    assert_eq!(response.tool_calls[1].arguments, json!("{\"path\": "));
    assert_eq!(
        (response.prompt_tokens, response.completion_tokens),
        (12, 3)
    );

    let unnamed = parse_completion(&json!({ "choices": [{ "message": { "content": "hi" } }] }));
    assert_eq!(
        unnamed.model, None,
        "a response naming no model must not be read as naming the one asked for"
    );
}
