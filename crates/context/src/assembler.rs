use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use vitna_tools::ToolDefinition;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssembledContext {
    pub messages: Vec<ContextMessage>,
    pub tools: Vec<ToolDefinition>,
    pub context_digest: String,
    pub estimated_tokens: usize,
}

pub struct ContextAssembler {
    pub system_prompt: String,
    pub max_token_budget: usize,
}

impl ContextAssembler {
    pub fn new() -> Self {
        Self {
            system_prompt: Self::default_system_prompt(),
            max_token_budget: 100_000,
        }
    }

    pub fn default_system_prompt() -> String {
        let mut prompt = String::new();
        prompt.push_str("You are Vitna Code, the local-first coding assistant that leaves a verifiable receipt.\n");
        prompt.push_str("Operating principles:\n");
        prompt.push_str("1. Local-first: You operate on the user's machine with zero cloud exfiltration.\n");
        prompt.push_str("2. Security outside the model: Propose actions cleanly; deterministic policy and sandbox enforce boundaries.\n");
        prompt.push_str("3. Evidence over confidence: Never claim work is done without running tests or checks.\n");
        prompt.push_str("4. Exact changes: When editing files, produce precise diffs with preimage and postimage awareness.\n");
        prompt.push_str("5. No em-dashes: Never use em-dash characters in source code or explanations.\n");
        prompt
    }

    /// Loads project-specific guidelines from workspace root (AGENTS.md, CLAUDE.md)
    /// and marks them as untrusted repository input.
    pub fn load_project_guidelines<P: AsRef<Path>>(workspace_root: P) -> Option<String> {
        let root = workspace_root.as_ref();
        let mut guidelines = String::new();

        let agents_path = root.join("AGENTS.md");
        if agents_path.exists() {
            if let Ok(content) = fs::read_to_string(&agents_path) {
                guidelines.push_str("<project_instructions source=\"AGENTS.md\">\n");
                guidelines.push_str(&content);
                guidelines.push_str("\n</project_instructions>\n");
            }
        }

        let claude_path = root.join("CLAUDE.md");
        if claude_path.exists() {
            if let Ok(content) = fs::read_to_string(&claude_path) {
                guidelines.push_str("<project_instructions source=\"CLAUDE.md\">\n");
                guidelines.push_str(&content);
                guidelines.push_str("\n</project_instructions>\n");
            }
        }

        if guidelines.is_empty() {
            None
        } else {
            Some(guidelines)
        }
    }

    /// Assembles full context into an ordered list of messages with tools and context digest.
    pub fn assemble<P: AsRef<Path>>(
        &self,
        workspace_root: P,
        history: &[ContextMessage],
        tools: &[ToolDefinition],
        active_diffs: Option<&str>,
    ) -> AssembledContext {
        let mut messages = Vec::new();

        // 1. System Prompt
        let mut full_system = self.system_prompt.clone();
        if let Some(guidelines) = Self::load_project_guidelines(workspace_root) {
            full_system.push_str("\nRepository Project Instructions (Untrusted Input):\n");
            full_system.push_str(&guidelines);
        }

        if let Some(diffs) = active_diffs {
            if !diffs.is_empty() {
                full_system.push_str("\nCurrent Uncommitted Workspace Diffs:\n");
                full_system.push_str(diffs);
                full_system.push('\n');
            }
        }

        messages.push(ContextMessage {
            role: "system".to_string(),
            content: full_system,
            tool_call_id: None,
            name: None,
        });

        // 2. Conversation History
        for msg in history {
            messages.push(msg.clone());
        }

        // 3. Compute estimated token count (rough heuristic: 4 chars = 1 token)
        let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        let estimated_tokens = total_chars / 4;

        // 4. Compute deterministic SHA-256 digest of assembled context
        let canonical_json = serde_json::to_string(&messages).unwrap_or_default();
        let hash = Sha256::digest(canonical_json.as_bytes());
        let context_digest = hex::encode(hash);

        AssembledContext {
            messages,
            tools: tools.to_vec(),
            context_digest,
            estimated_tokens,
        }
    }
}

impl Default for ContextAssembler {
    fn default() -> Self {
        Self::new()
    }
}
