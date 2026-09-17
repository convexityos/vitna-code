use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNode {
    pub task_id: String,
    pub role: String, // e.g. "researcher", "backend_engineer", "tester"
    pub prompt: String,
    pub dependencies: Vec<String>,
    pub status: TaskStatus,
    pub assigned_run_id: Option<String>,
    pub output_summary: Option<String>,
}

impl TaskNode {
    pub fn new(task_id: impl Into<String>, role: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            role: role.into(),
            prompt: prompt.into(),
            dependencies: Vec::new(),
            status: TaskStatus::Pending,
            assigned_run_id: None,
            output_summary: None,
        }
    }

    pub fn with_dependency(mut self, dep_task_id: impl Into<String>) -> Self {
        self.dependencies.push(dep_task_id.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraph {
    pub nodes: HashMap<String, TaskNode>,
}

impl TaskGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, node: TaskNode) -> Result<(), String> {
        if self.nodes.contains_key(&node.task_id) {
            return Err(format!("Duplicate task ID in graph: {}", node.task_id));
        }
        self.nodes.insert(node.task_id.clone(), node);
        Ok(())
    }

    /// Validates that all dependencies exist and that there are no cycles in the task graph.
    pub fn validate(&self) -> Result<(), String> {
        // 1. Verify dependency existence
        for (id, node) in &self.nodes {
            for dep in &node.dependencies {
                if !self.nodes.contains_key(dep) {
                    return Err(format!("Task '{}' depends on non-existent task '{}'", id, dep));
                }
            }
        }

        // 2. Cycle detection via Kahn's algorithm
        let mut in_degrees: HashMap<String, usize> = HashMap::new();
        for id in self.nodes.keys() {
            in_degrees.insert(id.clone(), 0);
        }

        for node in self.nodes.values() {
            for dep in &node.dependencies {
                if let Some(deg) = in_degrees.get_mut(dep) {
                    *deg += 1;
                }
            }
        }

        // Topological sort check
        let mut visited = 0;
        let mut queue: VecDeque<String> = self
            .nodes
            .keys()
            .filter(|id| self.nodes[*id].dependencies.is_empty())
            .cloned()
            .collect();

        let mut temp_deps: HashMap<String, HashSet<String>> = self
            .nodes
            .iter()
            .map(|(k, v)| (k.clone(), v.dependencies.iter().cloned().collect()))
            .collect();

        while let Some(current) = queue.pop_front() {
            visited += 1;
            for (id, deps) in temp_deps.iter_mut() {
                if deps.remove(&current) && deps.is_empty() {
                    queue.push_back(id.clone());
                }
            }
        }

        if visited != self.nodes.len() {
            return Err("Cycle detected in task graph dependencies".to_string());
        }

        Ok(())
    }

    /// Returns task IDs that are in Pending status and all of whose dependencies have Succeeded.
    pub fn get_ready_tasks(&self) -> Vec<String> {
        let mut ready = Vec::new();
        for (id, node) in &self.nodes {
            if node.status != TaskStatus::Pending {
                continue;
            }

            let all_deps_succeeded = node.dependencies.iter().all(|dep_id| {
                self.nodes
                    .get(dep_id)
                    .map(|d| d.status == TaskStatus::Succeeded)
                    .unwrap_or(false)
            });

            if all_deps_succeeded {
                ready.push(id.clone());
            }
        }
        ready.sort();
        ready
    }

    pub fn mark_running(&mut self, task_id: &str, run_id: &str) -> Result<(), String> {
        let node = self
            .nodes
            .get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;
        node.status = TaskStatus::Running;
        node.assigned_run_id = Some(run_id.to_string());
        Ok(())
    }

    pub fn mark_succeeded(&mut self, task_id: &str, output: &str) -> Result<(), String> {
        let node = self
            .nodes
            .get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;
        node.status = TaskStatus::Succeeded;
        node.output_summary = Some(output.to_string());
        Ok(())
    }

    /// Marks a task as failed and cancels all downstream tasks that depend on it.
    pub fn mark_failed(&mut self, task_id: &str, error: &str) -> Result<(), String> {
        let node = self
            .nodes
            .get_mut(task_id)
            .ok_or_else(|| format!("Task not found: {}", task_id))?;
        node.status = TaskStatus::Failed;
        node.output_summary = Some(format!("Error: {}", error));

        // Cascade cancellation to all dependent downstream nodes
        let mut to_cancel = VecDeque::new();
        to_cancel.push_back(task_id.to_string());

        while let Some(parent_id) = to_cancel.pop_front() {
            for (id, child) in self.nodes.iter_mut() {
                if child.dependencies.contains(&parent_id) && child.status == TaskStatus::Pending {
                    child.status = TaskStatus::Cancelled;
                    child.output_summary = Some(format!("Cancelled: prerequisite task '{}' failed", parent_id));
                    to_cancel.push_back(id.clone());
                }
            }
        }

        Ok(())
    }

    /// Returns true when all tasks are in a terminal state (Succeeded, Failed, or Cancelled).
    pub fn is_complete(&self) -> bool {
        self.nodes.values().all(|n| {
            matches!(
                n.status,
                TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
            )
        })
    }
}

impl Default for TaskGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_graph_lifecycle_and_cascading_cancellation() {
        let mut graph = TaskGraph::new();

        let t1 = TaskNode::new("research", "researcher", "Inspect codebase");
        let t2 = TaskNode::new("backend", "backend_engineer", "Implement API").with_dependency("research");
        let t3 = TaskNode::new("test", "tester", "Run test suite").with_dependency("backend");

        graph.add_node(t1).unwrap();
        graph.add_node(t2).unwrap();
        graph.add_node(t3).unwrap();

        graph.validate().expect("graph must be valid");

        // Initially only research is ready
        let ready1 = graph.get_ready_tasks();
        assert_eq!(ready1, vec!["research".to_string()]);

        // Mark research running then succeeded
        graph.mark_running("research", "run-101").unwrap();
        graph.mark_succeeded("research", "Analyzed architecture").unwrap();

        // Now backend is ready
        let ready2 = graph.get_ready_tasks();
        assert_eq!(ready2, vec!["backend".to_string()]);

        // Simulate backend failure: test must be cancelled automatically
        graph.mark_failed("backend", "Compilation failed").unwrap();
        assert_eq!(graph.nodes["test"].status, TaskStatus::Cancelled);
        assert!(graph.is_complete());
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = TaskGraph::new();
        let t1 = TaskNode::new("task1", "role1", "p1").with_dependency("task2");
        let t2 = TaskNode::new("task2", "role2", "p2").with_dependency("task1");

        graph.add_node(t1).unwrap();
        graph.add_node(t2).unwrap();

        assert!(graph.validate().is_err(), "Must detect cyclic dependency");
    }
}
