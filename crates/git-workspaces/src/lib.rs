//! Disposable agent workspaces and isolated checkout management.

pub mod host_git;
pub mod workspace;

pub use workspace::{AgentWorkspace, AgentWorkspaceManager};
