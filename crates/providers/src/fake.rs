//! Fake model provider adapter for deterministic testing, stream simulation, and fault injection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamItem {
    TextDelta(String),
    ToolCallDelta {
        tool_id: String,
        tool_name: String,
        argument_delta: String,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: u64,
        cost_usd: f64,
    },
    RateLimit {
        retry_after_secs: u64,
    },
    StreamError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeProviderConfig {
    pub provider_name: String,
    pub model_name: String,
    pub scenario: String,
    pub text_chunks: Vec<String>,
    pub tool_call: Option<(String, String, Vec<String>)>, // (id, name, chunks)
    pub simulate_rate_limit: bool,
    pub drop_after_chunk: Option<usize>,
}

impl Default for FakeProviderConfig {
    fn default() -> Self {
        Self {
            provider_name: "fake-provider".to_string(),
            model_name: "fake-model-v1".to_string(),
            scenario: "normal".to_string(),
            text_chunks: vec![
                "Analyzing ".to_string(),
                "codebase ".to_string(),
                "contracts.".to_string(),
            ],
            tool_call: None,
            simulate_rate_limit: false,
            drop_after_chunk: None,
        }
    }
}

pub struct FakeProvider {
    config: FakeProviderConfig,
}

impl FakeProvider {
    pub fn new(config: FakeProviderConfig) -> Self {
        Self { config }
    }

    pub fn stream_turn(&self) -> Vec<StreamItem> {
        let mut items = Vec::new();

        if self.config.simulate_rate_limit {
            items.push(StreamItem::RateLimit { retry_after_secs: 2 });
            return items;
        }

        if let Some((tool_id, tool_name, chunks)) = &self.config.tool_call {
            for (i, chunk) in chunks.iter().enumerate() {
                if let Some(drop_idx) = self.config.drop_after_chunk {
                    if i >= drop_idx {
                        items.push(StreamItem::StreamError("connection_reset_by_peer".to_string()));
                        return items;
                    }
                }
                items.push(StreamItem::ToolCallDelta {
                    tool_id: tool_id.clone(),
                    tool_name: tool_name.clone(),
                    argument_delta: chunk.clone(),
                });
            }
        } else {
            for (i, chunk) in self.config.text_chunks.iter().enumerate() {
                if let Some(drop_idx) = self.config.drop_after_chunk {
                    if i >= drop_idx {
                        items.push(StreamItem::StreamError("connection_reset_by_peer".to_string()));
                        return items;
                    }
                }
                items.push(StreamItem::TextDelta(chunk.clone()));
            }
        }

        items.push(StreamItem::Usage {
            prompt_tokens: 500,
            completion_tokens: 30,
            cached_tokens: 200,
            cost_usd: 0.0015,
        });

        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_streaming_simulation() {
        let provider = FakeProvider::new(FakeProviderConfig::default());
        let items = provider.stream_turn();

        assert_eq!(items.len(), 4); // 3 text deltas + 1 usage
        match &items[0] {
            StreamItem::TextDelta(t) => assert_eq!(t, "Analyzing "),
            _ => panic!("expected text delta"),
        }
    }

    #[test]
    fn test_tool_call_stream_simulation() {
        let config = FakeProviderConfig {
            tool_call: Some((
                "call-1".to_string(),
                "read_file".to_string(),
                vec!["{\"path\": ".to_string(), "\"Cargo.toml\"}".to_string()],
            )),
            ..Default::default()
        };

        let provider = FakeProvider::new(config);
        let items = provider.stream_turn();

        assert_eq!(items.len(), 3); // 2 tool deltas + 1 usage
        match &items[0] {
            StreamItem::ToolCallDelta { tool_name, .. } => assert_eq!(tool_name, "read_file"),
            _ => panic!("expected tool delta"),
        }
    }

    #[test]
    fn test_mid_stream_drop_simulation() {
        let config = FakeProviderConfig {
            text_chunks: vec!["chunk1 ".to_string(), "chunk2 ".to_string(), "chunk3".to_string()],
            drop_after_chunk: Some(2),
            ..Default::default()
        };

        let provider = FakeProvider::new(config);
        let items = provider.stream_turn();

        assert_eq!(items.len(), 3); // chunk1, chunk2, StreamError
        match &items[2] {
            StreamItem::StreamError(err) => assert_eq!(err, "connection_reset_by_peer"),
            _ => panic!("expected stream error"),
        }
    }

    #[test]
    fn test_rate_limit_simulation() {
        let config = FakeProviderConfig {
            simulate_rate_limit: true,
            ..Default::default()
        };

        let provider = FakeProvider::new(config);
        let items = provider.stream_turn();

        assert_eq!(items.len(), 1);
        match &items[0] {
            StreamItem::RateLimit { retry_after_secs } => assert_eq!(*retry_after_secs, 2),
            _ => panic!("expected rate limit"),
        }
    }
}
