//! Vitna Agent Protocol definitions, wire framing, and envelope serialization.

pub mod type_url;

use serde::{Deserialize, Serialize};
use std::io::{self, Read};

pub const PROTOCOL_VERSION_MAJOR: u32 = 1;
pub const PROTOCOL_VERSION_MINOR: u32 = 0;
pub const MAX_FRAME_SIZE_BYTES: u32 = 16 * 1024 * 1024; // 16 MB

/// The sequence number the first event of a run carries.
///
/// It is 1 rather than 0, and the reason is `SubscribeEvents`. Its
/// `resume_after_sequence` is a proto3 scalar, so an unset field and a literal
/// zero are the same value on the wire, and a first subscription therefore
/// cannot say "from the beginning" except by sending 0. "Resume AFTER 0" has
/// to mean "everything", which it does only while no event is numbered 0.
///
/// Numbering from 0 makes the run's opening event unreachable, and does it
/// silently: a subscriber's contiguity check needs a previous event to measure
/// against, so the one event it can never notice missing is the first.
///
/// `apps/vitna-desktop/src/client/client.ts` states the same reading from the
/// client side and opens every subscription with 0.
pub const FIRST_EVENT_SEQUENCE: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolEnvelope {
    pub protocol_version_major: u32,
    pub protocol_version_minor: u32,
    pub schema_version: u32,
    pub type_url: String,
    pub session_id: String,
    pub run_id: String,
    pub sequence: u64,
    pub idempotency_key: String,
    pub payload: Vec<u8>,
}

impl ProtocolEnvelope {
    pub fn new(
        type_url: impl Into<String>,
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        sequence: u64,
        idempotency_key: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            protocol_version_major: PROTOCOL_VERSION_MAJOR,
            protocol_version_minor: PROTOCOL_VERSION_MINOR,
            schema_version: 1,
            type_url: type_url.into(),
            session_id: session_id.into(),
            run_id: run_id.into(),
            sequence,
            idempotency_key: idempotency_key.into(),
            payload,
        }
    }

    /// Encodes the envelope with a 4-byte big-endian length prefix.
    pub fn encode_frame(&self) -> Result<Vec<u8>, io::Error> {
        let serialized = serde_json::to_vec(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let len = serialized.len() as u32;

        if len > MAX_FRAME_SIZE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Frame length {} exceeds maximum limit {}", len, MAX_FRAME_SIZE_BYTES),
            ));
        }

        let mut frame = Vec::with_capacity(4 + serialized.len());
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&serialized);
        Ok(frame)
    }

    /// Decodes a frame from a reader with a 4-byte big-endian length prefix.
    pub fn decode_frame<R: Read>(reader: &mut R) -> Result<Self, io::Error> {
        let mut len_bytes = [0u8; 4];
        reader.read_exact(&mut len_bytes)?;
        let frame_len = u32::from_be_bytes(len_bytes);

        if frame_len > MAX_FRAME_SIZE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Incoming frame size {} exceeds limit {}", frame_len, MAX_FRAME_SIZE_BYTES),
            ));
        }

        let mut buffer = vec![0u8; frame_len as usize];
        reader.read_exact(&mut buffer)?;

        let envelope: ProtocolEnvelope = serde_json::from_slice(&buffer)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_roundtrip() {
        let envelope = ProtocolEnvelope::new(
            "type.vitna.ai/vitna.protocol.v1.SubmitTurn",
            "session-001",
            "run-101",
            1,
            "idem-key-99",
            b"{\"prompt\": \"fix test\"}".to_vec(),
        );

        let frame = envelope.encode_frame().expect("encoding must succeed");
        assert!(frame.len() > 4);

        let mut cursor = std::io::Cursor::new(frame);
        let decoded = ProtocolEnvelope::decode_frame(&mut cursor).expect("decoding must succeed");

        assert_eq!(envelope, decoded);
    }

    #[test]
    fn test_frame_size_limit_enforced() {
        let mut huge_frame = Vec::new();
        let invalid_len = MAX_FRAME_SIZE_BYTES + 1;
        huge_frame.extend_from_slice(&invalid_len.to_be_bytes());

        let mut cursor = std::io::Cursor::new(huge_frame);
        let err = ProtocolEnvelope::decode_frame(&mut cursor).expect_err("must fail on oversized frame");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}