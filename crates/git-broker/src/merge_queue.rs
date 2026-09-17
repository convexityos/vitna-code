use crate::broker::{ChangeSet, GitBroker, MergeConflict};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueueItem {
    pub queue_id: String,
    pub task_id: String,
    pub agent_workspace_root: PathBuf,
    pub changeset: ChangeSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeQueueResult {
    Merged {
        applied_files: Vec<String>,
    },
    Conflict {
        conflicts: Vec<MergeConflict>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug)]
pub struct MergeQueue {
    pub primary_repo_root: PathBuf,
    pending: VecDeque<MergeQueueItem>,
    history: Vec<(MergeQueueItem, MergeQueueResult)>,
}

impl MergeQueue {
    pub fn new<P: Into<PathBuf>>(primary_repo_root: P) -> Self {
        Self {
            primary_repo_root: primary_repo_root.into(),
            pending: VecDeque::new(),
            history: Vec::new(),
        }
    }

    pub fn enqueue(&mut self, item: MergeQueueItem) {
        self.pending.push_back(item);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn history(&self) -> &[(MergeQueueItem, MergeQueueResult)] {
        &self.history
    }

    /// Processes the next item in the merge queue.
    /// Re-validates preimages against the current primary checkout before mutating anything.
    pub fn process_next(&mut self) -> Option<(String, MergeQueueResult)> {
        let item = self.pending.pop_front()?;
        let task_id = item.task_id.clone();

        // 1. Re-validate preimages against the current primary state
        let conflicts = match GitBroker::validate_preimages(&self.primary_repo_root, &item.changeset) {
            Ok(c) => c,
            Err(e) => {
                let res = MergeQueueResult::Error { message: e };
                self.history.push((item, res.clone()));
                return Some((task_id, res));
            }
        };

        if !conflicts.is_empty() {
            let res = MergeQueueResult::Conflict { conflicts };
            self.history.push((item, res.clone()));
            return Some((task_id, res));
        }

        // 2. Apply modifications to primary checkout
        let mut applied = Vec::new();
        for change in &item.changeset.files {
            let src = item.agent_workspace_root.join(&change.path);
            let dst = self.primary_repo_root.join(&change.path);

            if let Some(parent) = dst.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    let res = MergeQueueResult::Error { message: e.to_string() };
                    self.history.push((item, res.clone()));
                    return Some((task_id, res));
                }
            }

            if let Err(e) = fs::copy(&src, &dst) {
                let res = MergeQueueResult::Error { message: e.to_string() };
                self.history.push((item, res.clone()));
                return Some((task_id, res));
            }

            applied.push(change.path.clone());
        }

        let res = MergeQueueResult::Merged { applied_files: applied };
        self.history.push((item, res.clone()));
        Some((task_id, res))
    }

    /// Drain and process all pending queue items in order.
    pub fn process_all(&mut self) -> Vec<(String, MergeQueueResult)> {
        let mut results = Vec::new();
        while self.pending_count() > 0 {
            if let Some(res) = self.process_next() {
                results.push(res);
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitna_git_workspaces::AgentWorkspaceManager;

    #[test]
    fn test_merge_queue_sequential_and_conflict_handling() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_queue_test_{}", std::process::id()));
        fs::create_dir_all(temp_dir.join("src")).expect("create primary src");
        fs::write(temp_dir.join("src/a.rs"), "pub fn a() {}\n").expect("write a");
        fs::write(temp_dir.join("src/b.rs"), "pub fn b() {}\n").expect("write b");

        // Two agents branched from initial primary state
        let ws1 = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-agent-1")
            .expect("ws1");
        let ws2 = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-agent-2")
            .expect("ws2");

        // Agent 1 edits a.rs
        fs::write(ws1.agent_workspace_root.join("src/a.rs"), "pub fn a_updated() {}\n")
            .expect("edit a");
        let cs1 = GitBroker::inspect_changes(&ws1).expect("cs1");

        // Agent 2 edits BOTH a.rs (conflict with Agent 1) and b.rs
        fs::write(ws2.agent_workspace_root.join("src/a.rs"), "pub fn a_conflicting() {}\n")
            .expect("edit a conflicting");
        fs::write(ws2.agent_workspace_root.join("src/b.rs"), "pub fn b_updated() {}\n")
            .expect("edit b");
        let cs2 = GitBroker::inspect_changes(&ws2).expect("cs2");

        let mut queue = MergeQueue::new(&temp_dir);
        queue.enqueue(MergeQueueItem {
            queue_id: "q-1".to_string(),
            task_id: "task-1".to_string(),
            agent_workspace_root: ws1.agent_workspace_root.clone(),
            changeset: cs1,
        });
        queue.enqueue(MergeQueueItem {
            queue_id: "q-2".to_string(),
            task_id: "task-2".to_string(),
            agent_workspace_root: ws2.agent_workspace_root.clone(),
            changeset: cs2,
        });

        assert_eq!(queue.pending_count(), 2);

        // Process first item (Agent 1): should succeed
        let (task1_id, res1) = queue.process_next().expect("process 1");
        assert_eq!(task1_id, "task-1");
        match res1 {
            MergeQueueResult::Merged { applied_files } => {
                assert_eq!(applied_files, vec!["src/a.rs"]);
            }
            _ => panic!("Expected task-1 to merge cleanly"),
        }

        // Process second item (Agent 2): must fail with Conflict due to a.rs preimage mismatch
        let (task2_id, res2) = queue.process_next().expect("process 2");
        assert_eq!(task2_id, "task-2");
        match res2 {
            MergeQueueResult::Conflict { conflicts } => {
                assert_eq!(conflicts.len(), 1);
                assert_eq!(conflicts[0].path, "src/a.rs");
            }
            _ => panic!("Expected task-2 to have conflict"),
        }

        assert_eq!(queue.pending_count(), 0);
        assert_eq!(queue.history().len(), 2);

        // Clean up
        AgentWorkspaceManager::cleanup_workspace(&ws1).expect("clean 1");
        AgentWorkspaceManager::cleanup_workspace(&ws2).expect("clean 2");
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
