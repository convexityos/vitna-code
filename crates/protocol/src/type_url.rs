//! The `type_url` every payload travels under, as
//! `protocol/vitna/protocol/v1/*.proto` declares it.
//!
//! The envelope carries an opaque `payload` and a `type_url` saying what is in
//! it, so this module is the single place where a wire name is spelled. Both
//! ends of the wire read from here: the daemon to dispatch, a client to build.
//!
//! Every constant below names a message the `.proto` files declare, and every
//! message they declare has a constant, both directions held by
//! `tests/declared_protocol_conformance.rs`. That test exists because the two
//! halves of this repository had already drifted: an earlier daemon branch
//! spelled `Health`, `TurnResult`, `TurnList` and `Error`, none of which is
//! declared anywhere, while omitting ten commands that are. A name that
//! travels but is not declared cannot be implemented by a second client, and a
//! declared name nobody implements is a promise the protocol does not keep.
//!
//! [`ProtocolEnvelope`](crate::ProtocolEnvelope) is deliberately absent. It is
//! the frame itself rather than something framed, so it never appears in its
//! own `type_url`.

/// The prefix every declared name carries.
///
/// `apps/vitna-desktop/src/protocol/types.ts` spells this string too, and the
/// two must stay equal or no frame either end sends is readable by the other.
pub const PREFIX: &str = "type.vitna.ai/vitna.protocol.v1.";

/// Commands a client sends, from `commands.proto`.
pub mod command {
    pub const CREATE_SESSION: &str = "type.vitna.ai/vitna.protocol.v1.CreateSession";
    pub const SUBMIT_TURN: &str = "type.vitna.ai/vitna.protocol.v1.SubmitTurn";
    pub const STEER_RUN: &str = "type.vitna.ai/vitna.protocol.v1.SteerRun";
    pub const PAUSE_RUN: &str = "type.vitna.ai/vitna.protocol.v1.PauseRun";
    pub const RESUME_RUN: &str = "type.vitna.ai/vitna.protocol.v1.ResumeRun";
    pub const CANCEL_RUN: &str = "type.vitna.ai/vitna.protocol.v1.CancelRun";
    pub const APPROVE_ACTION: &str = "type.vitna.ai/vitna.protocol.v1.ApproveAction";
    pub const REJECT_ACTION: &str = "type.vitna.ai/vitna.protocol.v1.RejectAction";
    pub const SPAWN_CHILD: &str = "type.vitna.ai/vitna.protocol.v1.SpawnChild";
    pub const APPLY_CHANGE_SET: &str = "type.vitna.ai/vitna.protocol.v1.ApplyChangeSet";
    pub const ATTACH_TERMINAL: &str = "type.vitna.ai/vitna.protocol.v1.AttachTerminal";
    pub const SUBSCRIBE_EVENTS: &str = "type.vitna.ai/vitna.protocol.v1.SubscribeEvents";
}

/// Events the daemon emits, from `events.proto`.
pub mod event {
    pub const MESSAGE_DELTA: &str = "type.vitna.ai/vitna.protocol.v1.MessageDelta";
    pub const REASONING_SUMMARY_DELTA: &str =
        "type.vitna.ai/vitna.protocol.v1.ReasoningSummaryDelta";
    pub const PLAN_CHANGED: &str = "type.vitna.ai/vitna.protocol.v1.PlanChanged";
    pub const TOOL_PROPOSED: &str = "type.vitna.ai/vitna.protocol.v1.ToolProposed";
    pub const APPROVAL_REQUESTED: &str = "type.vitna.ai/vitna.protocol.v1.ApprovalRequested";
    pub const TOOL_STARTED: &str = "type.vitna.ai/vitna.protocol.v1.ToolStarted";
    pub const TOOL_OUTPUT: &str = "type.vitna.ai/vitna.protocol.v1.ToolOutput";
    pub const TOOL_FINISHED: &str = "type.vitna.ai/vitna.protocol.v1.ToolFinished";
    pub const DIFF_CHANGED: &str = "type.vitna.ai/vitna.protocol.v1.DiffChanged";
    pub const AGENT_SPAWNED: &str = "type.vitna.ai/vitna.protocol.v1.AgentSpawned";
    pub const AGENT_COMPLETED: &str = "type.vitna.ai/vitna.protocol.v1.AgentCompleted";
    pub const USAGE_UPDATED: &str = "type.vitna.ai/vitna.protocol.v1.UsageUpdated";
    pub const RUN_STATE_CHANGED: &str = "type.vitna.ai/vitna.protocol.v1.RunStateChanged";
    pub const DIAGNOSTIC: &str = "type.vitna.ai/vitna.protocol.v1.Diagnostic";
}

/// Messages declared in `envelope.proto` that travel as a payload like any
/// other. The handshake is a framed exchange, not a separate channel.
pub mod envelope {
    pub const HANDSHAKE_REQUEST: &str = "type.vitna.ai/vitna.protocol.v1.HandshakeRequest";
    pub const HANDSHAKE_RESPONSE: &str = "type.vitna.ai/vitna.protocol.v1.HandshakeResponse";
    pub const ERROR_RESPONSE: &str = "type.vitna.ai/vitna.protocol.v1.ErrorResponse";
}

/// Every command name, for dispatch tables and for the conformance test.
pub const COMMANDS: &[&str] = &[
    command::CREATE_SESSION,
    command::SUBMIT_TURN,
    command::STEER_RUN,
    command::PAUSE_RUN,
    command::RESUME_RUN,
    command::CANCEL_RUN,
    command::APPROVE_ACTION,
    command::REJECT_ACTION,
    command::SPAWN_CHILD,
    command::APPLY_CHANGE_SET,
    command::ATTACH_TERMINAL,
    command::SUBSCRIBE_EVENTS,
];

/// Every event name.
pub const EVENTS: &[&str] = &[
    event::MESSAGE_DELTA,
    event::REASONING_SUMMARY_DELTA,
    event::PLAN_CHANGED,
    event::TOOL_PROPOSED,
    event::APPROVAL_REQUESTED,
    event::TOOL_STARTED,
    event::TOOL_OUTPUT,
    event::TOOL_FINISHED,
    event::DIFF_CHANGED,
    event::AGENT_SPAWNED,
    event::AGENT_COMPLETED,
    event::USAGE_UPDATED,
    event::RUN_STATE_CHANGED,
    event::DIAGNOSTIC,
];

/// Every envelope-declared payload name.
pub const ENVELOPE_PAYLOADS: &[&str] = &[
    envelope::HANDSHAKE_REQUEST,
    envelope::HANDSHAKE_RESPONSE,
    envelope::ERROR_RESPONSE,
];

/// The declared message name a `type_url` carries, or `None` when the prefix
/// is not ours.
///
/// Returning `None` rather than the whole string is deliberate: an unknown
/// `type_url` is a frame this build cannot act on, and a caller that wants to
/// log the original still holds it.
pub fn message_name(type_url: &str) -> Option<&str> {
    type_url.strip_prefix(PREFIX)
}

/// Whether this build declares the name, in any of the three groups.
pub fn is_declared(type_url: &str) -> bool {
    COMMANDS.contains(&type_url)
        || EVENTS.contains(&type_url)
        || ENVELOPE_PAYLOADS.contains(&type_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_constant_carries_the_prefix_and_its_own_message_name() {
        for url in COMMANDS
            .iter()
            .chain(EVENTS.iter())
            .chain(ENVELOPE_PAYLOADS.iter())
        {
            let name = message_name(url).unwrap_or_else(|| panic!("{url} does not carry PREFIX"));
            assert!(
                !name.is_empty(),
                "{url} carries the prefix and nothing else"
            );
            assert!(
                !name.contains('.'),
                "{url} names something nested; the declared messages are all top level"
            );
        }
    }

    #[test]
    fn a_foreign_prefix_is_not_a_declared_name() {
        assert_eq!(message_name("type.example.com/Whatever"), None);
        assert!(!is_declared("type.example.com/Whatever"));
        assert!(is_declared(command::APPROVE_ACTION));
        assert!(is_declared(event::APPROVAL_REQUESTED));
    }

    #[test]
    fn no_name_is_declared_twice() {
        let mut all: Vec<&str> = COMMANDS
            .iter()
            .chain(EVENTS.iter())
            .chain(ENVELOPE_PAYLOADS.iter())
            .copied()
            .collect();
        let before = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(
            before,
            all.len(),
            "a type_url is listed in more than one group"
        );
    }
}
