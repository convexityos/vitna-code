//! Offline receipt verification engine for Vitna Code.

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
    pub errors: Vec<String>,
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

        // 5. Signature check (if public key provided)
        if let Some(pub_hex) = public_key_hex {
            let key_bytes = hex::decode(pub_hex)?;
            let verifying_key = VerifyingKey::from_bytes(
                key_bytes.as_slice().try_into().map_err(|_| "invalid key length")?
            )?;

            match receipt.verify_signature(&verifying_key) {
                Ok(true) => {}
                Ok(false) => errors.push("Device signature mismatch or payload tampered".to_string()),
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
            errors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitna_receipts::*;

    #[test]
    fn test_verifier_report() {
        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());

        let mut receipt = VitnaRunReceiptV1 {
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
            child_receipt_roots: vec![],
            device_signature: String::new(),
        };

        receipt.sign(&key).expect("signing");

        let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
        assert!(report.is_valid);
        assert_eq!(report.errors.len(), 0);
    }
}