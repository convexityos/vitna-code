//! Payload types for the declared messages, field for field.
//!
//! These mirror `protocol/vitna/protocol/v1/*.proto`. Field names are the
//! proto's own, because the framing is serde JSON and the field name IS the
//! wire format: renaming one here renames it on the wire and the desktop
//! client stops reading it.
//!
//! Proto3 default semantics are kept with `#[serde(default)]` on every field.
//! A missing field takes its type's default rather than failing, which is what
//! `apps/vitna-desktop/src/protocol/codec.ts` does on the other side. A field
//! of the WRONG type is still an error on both sides, never a guess.
//!
//! Only the messages the daemon serves today are here. The rest of the
//! declared set is named in [`crate::type_url`] and will arrive with the code
//! that answers it, rather than as empty structs that suggest a capability
//! nothing implements.

use serde::{Deserialize, Serialize};

/// `envelope.proto`: the version range a client can speak.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeRequest {
    #[serde(default)]
    pub min_supported_version: u32,
    #[serde(default)]
    pub max_supported_version: u32,
    #[serde(default)]
    pub client_identifier: String,
}

/// `envelope.proto`: what the daemon selected, or why it refused.
///
/// `accepted` is its own field rather than being inferred from
/// `rejection_reason` being empty, because "refused for a reason nobody wrote
/// down" has to stay expressible. A client reads `accepted` and nothing else
/// to decide whether it is connected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeResponse {
    #[serde(default)]
    pub selected_version_major: u32,
    #[serde(default)]
    pub selected_version_minor: u32,
    #[serde(default)]
    pub daemon_build_commit: String,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub rejection_reason: String,
}

/// `envelope.proto`'s `ErrorCode`, serialized as its declared name.
///
/// proto3 JSON writes an enum as its name, and
/// `codec.ts`'s `errorCode()` accepts either the name or the declaration-order
/// index. The name is chosen because it survives a reordering of the enum and
/// the index does not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[default]
    #[serde(rename = "ERROR_CODE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "ERROR_CODE_UNAUTHORIZED")]
    Unauthorized,
    #[serde(rename = "ERROR_CODE_INVALID_FRAME")]
    InvalidFrame,
    #[serde(rename = "ERROR_CODE_UNSUPPORTED_VERSION")]
    UnsupportedVersion,
    #[serde(rename = "ERROR_CODE_UNKNOWN_COMMAND")]
    UnknownCommand,
    #[serde(rename = "ERROR_CODE_POLICY_DENIED")]
    PolicyDenied,
    #[serde(rename = "ERROR_CODE_NEEDS_RECONCILIATION")]
    NeedsReconciliation,
    #[serde(rename = "ERROR_CODE_TIMEOUT")]
    Timeout,
    #[serde(rename = "ERROR_CODE_INTERNAL")]
    Internal,
}

/// `envelope.proto`: a refusal a caller never has to infer from a missing
/// field.
///
/// `action_id` is the only field the declaration gives for tying a refusal to
/// the thing refused, and the desktop client uses it to match an error to a
/// pending approval. Leave it empty when an error belongs to no particular
/// action; the client then shows it as a diagnostic rather than pinning it to
/// a guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    #[serde(default)]
    pub code: ErrorCode,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub error_details: String,
    #[serde(default)]
    pub action_id: String,
}

impl ErrorResponse {
    /// An error with no action behind it.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            error_details: String::new(),
            action_id: String::new(),
        }
    }
}

/// `commands.proto`: attach to a session's event stream.
///
/// `resume_after_sequence` is EXCLUSIVE: it names the last sequence the client
/// already holds, and the daemon sends what follows it. proto3 cannot tell an
/// unset field from a literal 0, so a first subscription sends 0 and must
/// receive everything, which is why no event is numbered 0. See
/// [`crate::FIRST_EVENT_SEQUENCE`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeEvents {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub resume_after_sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_code_travels_as_its_declared_name() {
        let json = serde_json::to_string(&ErrorCode::UnknownCommand).expect("serialize");
        assert_eq!(json, "\"ERROR_CODE_UNKNOWN_COMMAND\"");

        let back: ErrorCode = serde_json::from_str("\"ERROR_CODE_TIMEOUT\"").expect("deserialize");
        assert_eq!(back, ErrorCode::Timeout);
    }

    /// proto3 semantics: an absent field is its default, not a failure.
    #[test]
    fn a_missing_field_takes_its_proto3_default() {
        let sub: SubscribeEvents = serde_json::from_str("{}").expect("empty object decodes");
        assert_eq!(sub.session_id, "");
        assert_eq!(sub.resume_after_sequence, 0);

        let err: ErrorResponse = serde_json::from_str("{}").expect("empty object decodes");
        assert_eq!(err.code, ErrorCode::Unspecified);
    }

    /// A wrong type is still an error. proto3 defaults cover absence, never a
    /// value the sender got wrong.
    #[test]
    fn a_wrong_type_is_an_error_and_not_a_default() {
        let bad = serde_json::from_str::<SubscribeEvents>(r#"{"resume_after_sequence":"seven"}"#);
        assert!(bad.is_err(), "a string where a u64 belongs must not decode");
    }

    #[test]
    fn a_handshake_response_names_its_fields_as_the_declaration_does() {
        let value = serde_json::to_value(HandshakeResponse {
            selected_version_major: 1,
            selected_version_minor: 0,
            daemon_build_commit: "abc".into(),
            accepted: true,
            rejection_reason: String::new(),
        })
        .expect("serialize");

        for field in [
            "selected_version_major",
            "selected_version_minor",
            "daemon_build_commit",
            "accepted",
            "rejection_reason",
        ] {
            assert!(
                value.get(field).is_some(),
                "{field} is missing from the wire form"
            );
        }
    }
}
