//! Provider adapters with normalized event streaming and credential isolation.

pub mod anthropic;
pub mod credentials;
pub mod fake;
pub mod keyring;
pub mod openai;

pub use anthropic::AnthropicProvider;
use async_trait::async_trait;
pub use credentials::{CredentialResolver, ProviderCredentials};
pub use fake::{FakeProvider, FakeProviderConfig, ReplayEvent, StreamItem};
pub use keyring::KeyringStore;
pub use openai::OpenAIProvider;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<ProviderMessage>,
    pub tools: Vec<serde_json::Value>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderStreamEvent {
    TextDelta(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Finish(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub content: String,
    pub tool_calls: Vec<ProviderToolCall>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub finish_reason: String,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn parse_sse_event(&self, event_type: &str, data: &str) -> Result<Vec<ProviderStreamEvent>, String>;
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_sse_parsing() {
        let creds = ProviderCredentials {
            provider: "anthropic".to_string(),
            api_key: "sk-ant-test".to_string(),
            base_url: None,
        };
        let provider = AnthropicProvider::new(creds);

        // 1. Text delta
        let text_data = r#"{"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello world!"}}"#;
        let events = provider.parse_sse_event("content_block_delta", text_data).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ProviderStreamEvent::TextDelta("Hello world!".to_string()));

        // 2. Tool use start
        let tool_start = r#"{"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_01", "name": "read_file"}}"#;
        let events2 = provider.parse_sse_event("content_block_start", tool_start).unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(
            events2[0],
            ProviderStreamEvent::ToolCallDelta {
                index: 1,
                id: Some("toolu_01".to_string()),
                name: Some("read_file".to_string()),
                arguments_delta: String::new(),
            }
        );
    }

    #[test]
    fn test_openai_sse_parsing() {
        let creds = ProviderCredentials {
            provider: "openai".to_string(),
            api_key: "sk-proj-test".to_string(),
            base_url: None,
        };
        let provider = OpenAIProvider::new(creds);

        // 1. Text delta
        let data = r#"{"choices": [{"delta": {"content": "Thinking step by step..."}}]}"#;
        let events = provider.parse_sse_event("", data).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ProviderStreamEvent::TextDelta("Thinking step by step...".to_string()));

        // 2. Done token
        let done_events = provider.parse_sse_event("", "[DONE]").unwrap();
        assert_eq!(done_events.len(), 1);
        assert_eq!(done_events[0], ProviderStreamEvent::Finish("stop".to_string()));
    }
}