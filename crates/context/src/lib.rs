//! Context assembly, prompt engineering, token budgeting, and digest generation.

pub mod assembler;

pub use assembler::{AssembledContext, ContextAssembler, ContextMessage};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_context_assembler_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_context_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        // Write a test AGENTS.md
        fs::write(temp_dir.join("AGENTS.md"), "Strict test instructions.").expect("write AGENTS.md");

        let assembler = ContextAssembler::new();
        let history = vec![
            ContextMessage {
                role: "user".to_string(),
                content: "Please inspect src/lib.rs".to_string(),
                tool_call_id: None,
                name: None,
            },
        ];

        let tools = vec![];
        let diffs = "diff --git a/test b/test";

        let assembled = assembler.assemble(&temp_dir, &history, &tools, Some(diffs));

        assert_eq!(assembled.messages.len(), 2);
        assert_eq!(assembled.messages[0].role, "system");
        assert!(assembled.messages[0].content.contains("Strict test instructions."));
        assert!(assembled.messages[0].content.contains("diff --git"));
        assert_eq!(assembled.messages[1].role, "user");
        assert_eq!(assembled.context_digest.len(), 64);
        assert!(assembled.estimated_tokens > 0);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}