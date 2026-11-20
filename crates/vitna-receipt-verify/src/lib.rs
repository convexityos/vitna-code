//! Offline receipt verification engine for Vitna Code.

pub mod diff;
pub mod provenance;

pub use diff::{
    diff_receipts, highest_evidence_grade, CompletionDiff, EgressDiff, EvidenceDiff, FileDiff,
    IsolationDiff, MandateDiff, ModelDiff, ReceiptDiff,
};
pub use provenance::{
    audit_checkpoint, audit_envelope_json, compute_checkpoint_digest, verify_provenance_bundle,
    CheckpointAudit, EnvelopeAudit, ProvenanceAuditOptions, ProvenanceAuditReport,
};

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use vitna_receipts::VitnaRunReceiptV1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub is_valid: bool,
    pub run_id: String,
    pub schema_version: String,
    pub isolation_label: String,
    pub completion_state: String,
    pub evidence_count: usize,
    /// True only when a public key was supplied AND the device signature
    /// verified against it. A receipt checked without a key is never signed-off.
    pub signature_verified: bool,
    pub errors: Vec<String>,
}

/// A receipt parsed from JSON together with the verdict on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedReceipt {
    pub receipt: VitnaRunReceiptV1,
    pub report: VerificationReport,
}

/// Reads a receipt from JSON and checks everything checkable without a key.
/// No key is supplied on this path, so the device signature is NOT checked and
/// `report.signature_verified` is false. Callers must not present the result as
/// a verified signature. Use `ReceiptVerifier::verify_receipt` with a key for that.
pub fn verify_receipt_json(content: &str) -> Result<VerifiedReceipt, Box<dyn std::error::Error>> {
    let receipt: VitnaRunReceiptV1 = serde_json::from_str(content)?;
    let report = ReceiptVerifier::verify_receipt(&receipt, None)?;
    Ok(VerifiedReceipt { receipt, report })
}

pub struct ReceiptVerifier;

impl ReceiptVerifier {
    pub fn verify_file<P: AsRef<Path>>(
        receipt_path: P,
        public_key_hex: Option<&str>,
    ) -> Result<VerificationReport, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(receipt_path)?;
        let receipt: VitnaRunReceiptV1 = serde_json::from_str(&content)?;
        Self::verify_receipt(&receipt, public_key_hex)
    }

    pub fn verify_receipt(
        receipt: &VitnaRunReceiptV1,
        public_key_hex: Option<&str>,
    ) -> Result<VerificationReport, Box<dyn std::error::Error>> {
        let mut errors = Vec::new();

        // 1. Schema check
        if receipt.schema_version != "vitna-run-receipt-v1" {
            errors.push(format!("Unsupported schema version: {}", receipt.schema_version));
        }

        // 2. Isolation label check
        let valid_labels = ["read_only", "guarded", "strong", "full_access"];
        if !valid_labels.contains(&receipt.isolation_label.as_str()) {
            errors.push(format!("Unknown isolation label: {}", receipt.isolation_label));
        }

        // 3. Completion state check
        let valid_states = [
            "completed_with_evidence",
            "completed_with_unknowns",
            "blocked",
            "failed",
            "cancelled",
            "needs_reconciliation",
        ];
        if !valid_states.contains(&receipt.completion_state.as_str()) {
            errors.push(format!("Unknown completion state: {}", receipt.completion_state));
        }

        // 4. File preimage/postimage completeness check
        for file_mod in &receipt.changeset.files_modified {
            if file_mod.preimage_hash.is_empty() || file_mod.postimage_hash.is_empty() {
                errors.push(format!("Missing preimage or postimage hash for {}", file_mod.path));
            }
        }

        // 4b. Brokered egress record completeness check
        for (idx, call) in receipt.egress_calls.iter().enumerate() {
            if call.target_domain.trim().is_empty() {
                errors.push(format!("Egress call #{idx} is missing target_domain"));
            }
            if call.method.trim().is_empty() {
                errors.push(format!("Egress call #{idx} is missing HTTP method"));
            }
            if call.redacted_url.trim().is_empty() {
                errors.push(format!("Egress call #{idx} is missing redacted_url"));
            }
            if !(100..=599).contains(&call.status_code) {
                errors.push(format!(
                    "Egress call #{idx} has invalid HTTP status code {}",
                    call.status_code
                ));
            }
            let valid_sha256 = |s: &str| {
                s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
            };
            if !valid_sha256(&call.request_sha256) {
                errors.push(format!("Egress call #{idx} has invalid request_sha256"));
            }
            if !valid_sha256(&call.response_sha256) {
                errors.push(format!("Egress call #{idx} has invalid response_sha256"));
            }
        }

        // 5. Signature check (if public key provided)
        let mut signature_verified = false;
        if let Some(pub_hex) = public_key_hex {
            let trimmed_pub = pub_hex.trim();
            let key_bytes = hex::decode(trimmed_pub)?;
            let verifying_key = VerifyingKey::from_bytes(
                key_bytes.as_slice().try_into().map_err(|_| "invalid key length")?
            )?;

            let key_mismatch = receipt
                .device_public_key
                .as_deref()
                .map(|embedded| !embedded.trim().eq_ignore_ascii_case(trimmed_pub))
                .unwrap_or(false);

            match receipt.verify_signature(&verifying_key) {
                Ok(true) => {
                    if key_mismatch {
                        errors.push(
                            "Receipt was signed by a different device key (embedded key does not match supplied public key)"
                                .to_string(),
                        );
                    } else {
                        signature_verified = true;
                    }
                }
                Ok(false) => {
                    if key_mismatch {
                        errors.push(
                            "Receipt was signed by a different device key (embedded key does not match supplied public key)"
                                .to_string(),
                        );
                    } else {
                        errors.push("Device signature mismatch or payload tampered".to_string());
                    }
                }
                Err(e) => errors.push(format!("Signature verification error: {}", e)),
            }
        }

        let is_valid = errors.is_empty();

        Ok(VerificationReport {
            is_valid,
            run_id: receipt.run_id.clone(),
            schema_version: receipt.schema_version.clone(),
            isolation_label: receipt.isolation_label.clone(),
            completion_state: receipt.completion_state.clone(),
            evidence_count: receipt.evidence_items.len(),
            signature_verified,
            errors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;
    use vitna_receipts::*;

    fn sample_receipt() -> VitnaRunReceiptV1 {
        VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-099".to_string(),
            session_id: "sess-099".to_string(),
            workspace_fingerprint: "ws-fp-99".to_string(),
            base_commit_sha: "1234567890123456789012345678901234567890".to_string(),
            model_selection: ModelSelectionRecord {
                provider: "openai".to_string(),
                model_sku: "gpt-5".to_string(),
                routing_reason: "policy".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "merkle-root-99".to_string(),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![],
            changeset: ChangeSetRecord {
                files_modified: vec![],
                diff_digest: "diff-99".to_string(),
            },
            runner_execution_statements: vec![],
            egress_calls: vec![],
            child_receipt_roots: vec![],
            device_public_key: None,
            mandate_provenance: None,
            device_signature: String::new(),
        }
    }

    #[test]
    fn test_verifier_report() {
        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());

        let mut receipt = sample_receipt();
        receipt.sign(&key).expect("signing");
        assert_eq!(receipt.device_public_key.as_deref(), Some(pub_hex.as_str()));

        let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(report.is_valid);
        assert!(report.signature_verified);
        assert_eq!(report.errors.len(), 0);
    }

    #[test]
    fn test_verifier_distinguishes_wrong_key_from_tampered_payload() {
        let signing_key = generate_signing_key();
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let other_key = generate_signing_key();
        let other_pub_hex = hex::encode(other_key.verifying_key().to_bytes());

        let mut receipt = sample_receipt();
        receipt.sign(&signing_key).expect("signing");

        // Wrong key: distinct error naming different device key.
        let wrong_key_report =
            ReceiptVerifier::verify_receipt(&receipt, Some(&other_pub_hex)).expect("verify");
        assert!(!wrong_key_report.is_valid);
        assert!(!wrong_key_report.signature_verified);
        assert!(
            wrong_key_report
                .errors
                .iter()
                .any(|e| e.contains("different device key")),
            "expected wrong-key diagnostic, got {:?}",
            wrong_key_report.errors
        );

        // Matching key, tampered payload: reports signature mismatch / payload tampered.
        receipt.changeset.diff_digest = "tampered".to_string();
        let tampered_report =
            ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(!tampered_report.is_valid);
        assert!(!tampered_report.signature_verified);
        assert!(
            tampered_report
                .errors
                .iter()
                .any(|e| e.contains("payload tampered")),
            "expected tamper diagnostic, got {:?}",
            tampered_report.errors
        );
    }

    #[test]
    fn test_legacy_receipt_without_device_public_key_still_verifies() {
        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());

        let mut receipt = sample_receipt();
        let bytes = receipt.to_canonical_bytes().expect("canonical");
        assert!(
            !String::from_utf8_lossy(&bytes).contains("device_public_key"),
            "legacy receipt without device_public_key must omit it from canonical bytes"
        );
        assert!(
            !String::from_utf8_lossy(&bytes).contains("egress_calls"),
            "receipt with empty egress_calls must omit it from canonical bytes"
        );
        receipt.device_signature = hex::encode(key.sign(&bytes).to_bytes());

        let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(report.is_valid);
        assert!(report.signature_verified);
    }

    #[test]
    fn test_verifier_validates_and_signs_egress_calls() {
        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());

        let mut receipt = sample_receipt();
        receipt.egress_calls.push(EgressCallRecord {
            target_domain: "127.0.0.1".to_string(),
            method: "GET".to_string(),
            redacted_url: "http://127.0.0.1:8080/health".to_string(),
            request_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .to_string(),
            status_code: 200,
            response_sha256: "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"
                .to_string(),
            bytes_sent: 0,
            bytes_received: 5,
            latency_ms: 4,
        });
        receipt.sign(&key).expect("signing");

        let ok = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(ok.is_valid, "errors: {:?}", ok.errors);
        assert!(ok.signature_verified);

        // Tampering an egress record invalidates both structure/signature.
        receipt.egress_calls[0].response_sha256 = "not-a-sha256".to_string();
        let bad = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(!bad.is_valid);
        assert!(!bad.signature_verified);
        assert!(bad
            .errors
            .iter()
            .any(|e| e.contains("invalid response_sha256")));
    }
}