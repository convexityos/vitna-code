//! Event names this engine records for its own account, which the Vitna Agent
//! Protocol does not declare.
//!
//! The event log is the receipt's evidence, so it holds more than a client is
//! meant to render. These names are deliberately NOT in `vitna_protocol::
//! type_url`, and cannot be: the conformance test there requires every constant
//! to name a message some `.proto` declares, so adding one of these would fail
//! it. That is the intended outcome, not an obstacle.
//!
//! They carry their own prefix for one reason. Written as `vitna.v1.X` beside a
//! declared `type.vitna.ai/vitna.protocol.v1.X`, they look like a botched
//! prefix, and the obvious repair is to "fix" them onto the declared one. That
//! repair would put an undeclared name on the wire, where a client decodes it
//! as a declared message with every field empty. `vitna.audit.v1.X` says the
//! prefix is a decision.
//!
//! These are still STREAMED, like every other event. A subscriber numbers a
//! session's events with one counter, so withholding one leaves a hole that
//! reads as a gap, and the client resubscribes into it forever. A client's job
//! is to admit an event it cannot decode and keep counting; see
//! `crates/daemon/tests/ipc_subscription.rs`.

/// The prefix that marks an event as this engine's own record.
pub const PREFIX: &str = "vitna.audit.v1.";

/// A turn began, carrying the prompt. The only place a prompt is recorded: a
/// receipt attests to what was DONE, not to what was asked.
pub const TURN_STARTED: &str = "vitna.audit.v1.TurnStarted";

/// An approval was granted. Deliberately not a declared event.
///
/// The declared protocol proves an approval took effect with `ToolStarted`,
/// and the desktop client marks an approval approved only then, raising an
/// alarm if a tool it rejected starts anyway. "Granted" is a claim about an
/// intention; a tool starting is a fact. Streaming a claim a client could
/// render as an outcome would let it show "approved" for something that never
/// ran, so this stays evidence in the log rather than a promise on the wire.
pub const APPROVAL_GRANTED: &str = "vitna.audit.v1.ApprovalGranted";

/// What the model was given, by digest and size. Evidence for the receipt.
pub const CONTEXT_ASSEMBLED: &str = "vitna.audit.v1.ContextAssembled";

/// A receipt was written for this run.
pub const RECEIPT_GENERATED: &str = "vitna.audit.v1.ReceiptGenerated";

/// Every audit name, so a test can check the set rather than a list someone
/// remembered to update.
pub const ALL: &[&str] = &[
    TURN_STARTED,
    APPROVAL_GRANTED,
    CONTEXT_ASSEMBLED,
    RECEIPT_GENERATED,
];

/// Whether `type_url` is one of this engine's own names.
pub fn is_audit(type_url: &str) -> bool {
    ALL.contains(&type_url)
}
