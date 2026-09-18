//! Trusted Git broker managing isolated agent changesets, preimage verification, and safe merging.

pub mod broker;
pub mod merge_queue;
mod tree;

pub use broker::{ChangeSet, FileChange, GitBroker, MergeConflict, MergeResult, NULL_HASH};
pub use merge_queue::{MergeQueue, MergeQueueItem, MergeQueueResult};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use vitna_git_workspaces::AgentWorkspaceManager;

    #[test]
    fn test_git_broker_changeset_and_merge() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_broker_test_{}", std::process::id()));
        fs::create_dir_all(temp_dir.join("src")).expect("create primary src");
        fs::write(temp_dir.join("src/lib.rs"), "pub fn original() {}\n").expect("write original");

        // 1. Create isolated agent workspace
        let ws = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-42")
            .expect("create workspace");

        // 2. Modify file in agent workspace
        fs::write(ws.agent_workspace_root.join("src/lib.rs"), "pub fn modified() {}\n")
            .expect("write modified");

        // 3. Inspect changes
        let changeset = GitBroker::inspect_changes(&ws).expect("inspect changes");
        assert_eq!(changeset.files.len(), 1);
        assert_eq!(changeset.files[0].path, "src/lib.rs");
        assert_ne!(changeset.files[0].preimage_hash, changeset.files[0].postimage_hash);

        // 4. Validate preimages: initially no conflict
        let conflicts = GitBroker::validate_preimages(&temp_dir, &changeset).expect("validate");
        assert!(conflicts.is_empty());

        // 5. Simulate concurrent primary modification (conflict test)
        fs::write(temp_dir.join("src/lib.rs"), "pub fn concurrent_change() {}\n").expect("concurrent");
        let conflicts2 = GitBroker::validate_preimages(&temp_dir, &changeset).expect("validate");
        assert_eq!(conflicts2.len(), 1);
        assert_eq!(conflicts2[0].path, "src/lib.rs");

        // Attempting apply_changeset must fail due to conflict
        let merge_res = GitBroker::apply_changeset(&temp_dir, &ws, &changeset).expect("apply");
        assert!(!merge_res.success);
        assert_eq!(merge_res.conflicts.len(), 1);

        // 6. Reset primary to original preimage and apply cleanly
        fs::write(temp_dir.join("src/lib.rs"), "pub fn original() {}\n").expect("reset");
        let merge_clean = GitBroker::apply_changeset(&temp_dir, &ws, &changeset).expect("apply clean");
        assert!(merge_clean.success);
        assert_eq!(merge_clean.applied_files, vec!["src/lib.rs"]);

        // Clean up
        AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup");
        let _ = fs::remove_dir_all(&temp_dir);
    }
}