//! Cryptographic receipt generation, Merkle root calculation, and canonical signing.

pub mod manifest;

pub use manifest::{
    ManifestVerificationReport, ReleaseAsset, VitnaReleaseManifestV1, RELEASE_MANIFEST_SCHEMA_V1,
    SUPPORTED_RELEASE_CHANNELS, SUPPORTED_RELEASE_TARGETS,
};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use vitna_policy::mandate::MandateProvenanceRecord;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelectionRecord {
    pub provider: String,
    pub model_sku: String,
    pub routing_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItemRecord {
    pub evidence_id: String,
    pub grade: String, // "model_reported", "broker_observed", "sandbox_captured", "independently_reproduced", "remote_or_hardware_attested"
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileModificationRecord {
    pub path: String,
    pub preimage_hash: String,
    pub postimage_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSetRecord {
    pub files_modified: Vec<FileModificationRecord>,
    pub diff_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerExecutionStatementRecord {
    pub action_id: String,
    pub statement_digest: String,
    pub signature: String,
    /// The sandbox mechanism that confined this action: `bubblewrap`,
    /// `seatbelt`, `appcontainer`, or `none`.
    ///
    /// Omitted when absent so that receipts written before this field existed
    /// still produce the same canonical bytes and still verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_backend: Option<String>,
    /// `fully_enforced`, `fallback`, or `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_enforcement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VitnaRunReceiptV1 {
    pub schema_version: String,
    pub run_id: String,
    pub session_id: String,
    pub workspace_fingerprint: String,
    pub base_commit_sha: String,
    pub model_selection: ModelSelectionRecord,
    pub event_hash_chain_root: String,
    pub isolation_label: String, // "read_only", "guarded", "strong", "full_access"
    pub completion_state: String, // "completed_with_evidence", "completed_with_unknowns", "blocked", "failed", "cancelled", "needs_reconciliation"
    pub evidence_items: Vec<EvidenceItemRecord>,
    pub changeset: ChangeSetRecord,
    pub runner_execution_statements: Vec<RunnerExecutionStatementRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_receipt_roots: Vec<String>,
    /// Hex-encoded 32-byte Ed25519 verifying key that signed this receipt.
    /// Omitted on legacy receipts written before this field existed so their
    /// canonical bytes remain unchanged and continue to verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_public_key: Option<String>,
    /// Optional `am/1` Agent Mandate provenance for mandated autonomous runs.
    ///
    /// Omitted when absent so receipts without mandates produce identical
    /// canonical bytes and existing fixtures continue to verify unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandate_provenance: Option<MandateProvenanceRecord>,
    #[serde(default)]
    pub device_signature: String,
}

impl VitnaRunReceiptV1 {
    /// Aggregates child agent evidence and event roots into this parent receipt.
    pub fn aggregate_child_receipt(&mut self, child_receipt: &VitnaRunReceiptV1) {
        if !child_receipt.event_hash_chain_root.is_empty() {
            self.child_receipt_roots.push(child_receipt.event_hash_chain_root.clone());
        }
        for item in &child_receipt.evidence_items {
            if !self.evidence_items.iter().any(|e| e.evidence_id == item.evidence_id) {
                self.evidence_items.push(item.clone());
            }
        }
    }
    /// Computes canonical JSON representation (RFC 8785 discipline: deterministic key ordering, compact whitespace).
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut clone = self.clone();
        clone.device_signature.clear(); // Signature does not cover itself
        let value = serde_json::to_value(&clone)?;
        let canonical_str = Self::canonicalize_value(&value);
        Ok(canonical_str.into_bytes())
    }

    fn canonicalize_value(val: &serde_json::Value) -> String {
        match val {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => serde_json::to_string(s).unwrap(),
            serde_json::Value::Array(arr) => {
                let items: Vec<String> = arr.iter().map(Self::canonicalize_value).collect();
                format!("[{}]", items.join(","))
            }
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let entries: Vec<String> = keys
                    .iter()
                    .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), Self::canonicalize_value(&map[*k])))
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
        }
    }

    /// Signs the receipt with an Ed25519 signing key.
    pub fn sign(&mut self, key: &SigningKey) -> Result<(), serde_json::Error> {
        self.device_public_key = Some(hex::encode(key.verifying_key().to_bytes()));
        let bytes = self.to_canonical_bytes()?;
        let signature = key.sign(&bytes);
        self.device_signature = hex::encode(signature.to_bytes());
        Ok(())
    }

    /// Verifies the device signature using the provided public verifying key.
    pub fn verify_signature(&self, key: &VerifyingKey) -> Result<bool, Box<dyn std::error::Error>> {
        let bytes = self.to_canonical_bytes()?;
        let sig_bytes = hex::decode(&self.device_signature)?;
        let sig = Signature::from_slice(&sig_bytes)?;
        Ok(key.verify_strict(&bytes, &sig).is_ok())
    }

    /// Computes Merkle root over an array of event hashes.
    pub fn compute_event_merkle_root(event_hashes: &[String]) -> String {
        if event_hashes.is_empty() {
            return "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        }

        let mut current_level: Vec<Vec<u8>> = event_hashes
            .iter()
            .map(|h| hex::decode(h).unwrap_or_else(|_| vec![0u8; 32]))
            .collect();

        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&chunk[0]);
                if chunk.len() > 1 {
                    hasher.update(&chunk[1]);
                } else {
                    hasher.update(&chunk[0]);
                }
                next_level.push(hasher.finalize().to_vec());
            }
            current_level = next_level;
        }

        hex::encode(&current_level[0])
    }
}

pub fn generate_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_receipt_canonicalization_and_signing() {
        let key = generate_signing_key();
        let verifying_key = key.verifying_key();

        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-001".to_string(),
            session_id: "sess-001".to_string(),
            workspace_fingerprint: "ws-fp-123".to_string(),
            base_commit_sha: "abcd1234abcd1234abcd1234abcd1234abcd1234".to_string(),
            model_selection: ModelSelectionRecord {
                provider: "anthropic".to_string(),
                model_sku: "claude-3-7-sonnet".to_string(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: Some("policy-hash-456".to_string()),
            },
            event_hash_chain_root: "merkle-root-789".to_string(),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![EvidenceItemRecord {
                evidence_id: "ev-1".to_string(),
                grade: "sandbox_captured".to_string(),
                description: "Targeted pytest passed cleanly".to_string(),
                artifact_digest: Some("art-hash-111".to_string()),
            }],
            changeset: ChangeSetRecord {
                files_modified: vec![FileModificationRecord {
                    path: "src/main.rs".to_string(),
                    preimage_hash: "pre-1".to_string(),
                    postimage_hash: "post-1".to_string(),
                }],
                diff_digest: "diff-hash-222".to_string(),
            },
            runner_execution_statements: vec![RunnerExecutionStatementRecord {
                action_id: "act-1".to_string(),
                statement_digest: "stmt-digest-1".to_string(),
                signature: "sig-1".to_string(),
                sandbox_backend: None,
                sandbox_enforcement: None,
            }],
            child_receipt_roots: Vec::new(),
            device_public_key: None,
            mandate_provenance: None,
            device_signature: String::new(),
        };

        let legacy_canonical = String::from_utf8(receipt.to_canonical_bytes().unwrap()).unwrap();
        assert!(!legacy_canonical.contains("device_public_key"));
        assert!(!legacy_canonical.contains("mandate_provenance"));

        receipt.sign(&key).expect("signing must succeed");
        assert!(!receipt.device_signature.is_empty());
        assert_eq!(
            receipt.device_public_key.as_deref(),
            Some(hex::encode(verifying_key.to_bytes()).as_str())
        );

        let is_valid = receipt.verify_signature(&verifying_key).expect("verification check");
        assert!(is_valid);

        // Tamper test: modify diff_digest
        receipt.changeset.diff_digest = "tampered-digest".to_string();
        let is_tampered_valid = receipt.verify_signature(&verifying_key).unwrap_or(false);
        assert!(!is_tampered_valid);
    }

    #[test]
    fn test_receipt_mandate_provenance_signing_and_tamper_detection() {
        let key = generate_signing_key();
        let verifying_key = key.verifying_key();

        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-mandated-01".to_string(),
            session_id: "sess-mandated-01".to_string(),
            workspace_fingerprint: "ws-fp-mandated".to_string(),
            base_commit_sha: "abcd1234abcd1234abcd1234abcd1234abcd1234".to_string(),
            model_selection: ModelSelectionRecord {
                provider: "anthropic".to_string(),
                model_sku: "claude-3-7-sonnet".to_string(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "merkle-root-mandated".to_string(),
            isolation_label: "strong".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: Vec::new(),
            changeset: ChangeSetRecord {
                files_modified: Vec::new(),
                diff_digest: "diff-mandated".to_string(),
            },
            runner_execution_statements: Vec::new(),
            child_receipt_roots: Vec::new(),
            device_public_key: None,
            mandate_provenance: Some(MandateProvenanceRecord {
                chain_hash: "sha256:abc123".to_string(),
                root_jti: "mnd_root".to_string(),
                path: vec!["mnd_root".to_string(), "mnd_child".to_string()],
                ledger: "https://ledger.convexityos.com".to_string(),
                currency: "USD".to_string(),
                total_spent: "0.45".to_string(),
                spend_digests: vec!["sha256:spend1".to_string(), "sha256:spend2".to_string()],
            }),
            device_signature: String::new(),
        };

        receipt.sign(&key).expect("signing mandated receipt succeeds");
        assert!(receipt.verify_signature(&verifying_key).expect("verify succeeds"));

        // Tampering with total_spent must invalidate the signature.
        receipt.mandate_provenance.as_mut().unwrap().total_spent = "0.01".to_string();
        assert!(!receipt.verify_signature(&verifying_key).unwrap_or(false));
    }
}