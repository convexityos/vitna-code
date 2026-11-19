//! Who answers when an action needs a yes.
//!
//! Until the daemon served turns, an approval had two outcomes: the run was
//! configured to auto-approve, or it aborted with "Action requires human
//! approval". Nobody could be asked, because nothing could wait for an
//! answer. An [`Approver`] is the thing that waits. The engine asks it about
//! every mutating action and records what it was told.
//!
//! An engine with no approver keeps the old behaviour exactly, so `vitna run`
//! is unchanged: `auto_approve` grants, and anything else refuses.

use async_trait::async_trait;

/// One action waiting for a decision, as the operator is shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Unique per request. The operator's answer names it.
    pub approval_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    /// What the answer binds to. An approval for a different digest is not an
    /// approval of this action (CONTRIBUTING, rule 3).
    pub action_digest: String,
    pub description: String,
    pub canonical_cwd: String,
    /// How long the request stays open before it is refused. Zero is never
    /// sent: a request with no deadline would hold a run forever.
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    /// Refused, with the reason the model is told.
    Rejected(String),
}

#[async_trait]
pub trait Approver: Send + Sync {
    /// Whether a person is asked. When true the engine announces the request
    /// (`ApprovalRequested`) and moves the run to `waiting_for_approval`
    /// before calling [`decide`](Approver::decide); an approver that answers
    /// by rule is not shown as asking anybody.
    fn asks_a_person(&self) -> bool;

    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision;
}

/// Answers by rule: everything, or nothing.
pub struct RuleApprover {
    pub approve: bool,
}

#[async_trait]
impl Approver for RuleApprover {
    fn asks_a_person(&self) -> bool {
        false
    }

    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        if self.approve {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Rejected(format!(
                "Action requires human approval: {} (digest: {})",
                request.tool_name, request.action_digest
            ))
        }
    }
}
