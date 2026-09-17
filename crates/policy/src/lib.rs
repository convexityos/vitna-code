//! Execution policy engine, exact-action digests, capability checks, and signed model policies.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLaneConfig {
    pub lane: String, // e.g., "planning", "coding", "fast_eval"
    pub provider: String, // e.g., "anthropic", "fake"
    pub model_sku: String, // e.g., "claude-3-7-sonnet"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedModelPolicy {
    pub schema_version: String, // "vitna-model-policy-v1"
    pub policy_id: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub allowed_providers: Vec<String>,
    pub lanes: Vec<ModelLaneConfig>,
    pub required_sandbox_guarantee: String, // "read_only", "guarded", "strong", "full_access"
    pub public_key: String, // hex encoded Ed25519 public key
    #[serde(default)]
    pub signature: String,  // hex encoded Ed25519 signature over canonical payload
}

impl SignedModelPolicy {
    /// Computes canonical JSON representation (RFC 8785 discipline: deterministic key ordering, compact whitespace).
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut clone = self.clone();
        clone.signature.clear(); // Signature does not cover itself
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

    /// Computes the SHA-256 digest of the canonical policy.
    pub fn policy_digest(&self) -> Result<String, serde_json::Error> {
        let bytes = self.to_canonical_bytes()?;
        let hash = Sha256::digest(&bytes);
        Ok(hex::encode(hash))
    }

    /// Signs the policy with the corresponding signing key and updates the public key and signature fields.
    pub fn sign(&mut self, key: &SigningKey) -> Result<(), serde_json::Error> {
        self.public_key = hex::encode(key.verifying_key().to_bytes());
        let bytes = self.to_canonical_bytes()?;
        let sig = key.sign(&bytes);
        self.signature = hex::encode(sig.to_bytes());
        Ok(())
    }

    /// Verifies the signature against the embedded public key.
    pub fn verify_signature(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let pubkey_bytes = hex::decode(&self.public_key)?;
        let pubkey_array: [u8; 32] = pubkey_bytes
            .try_into()
            .map_err(|_| "Invalid public key length")?;
        let verifying_key = VerifyingKey::from_bytes(&pubkey_array)?;

        let bytes = self.to_canonical_bytes()?;
        let sig_bytes = hex::decode(&self.signature)?;
        let sig = Signature::from_slice(&sig_bytes)?;

        Ok(verifying_key.verify_strict(&bytes, &sig).is_ok())
    }

    /// Returns true if current_time exceeds expires_at.
    pub fn is_expired(&self, current_time: u64) -> bool {
        current_time > self.expires_at
    }
}

pub fn generate_policy_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signed_model_policy_lifecycle() {
        let key = generate_policy_signing_key();
        let mut policy = SignedModelPolicy {
            schema_version: "vitna-model-policy-v1".to_string(),
            policy_id: "test-policy-01".to_string(),
            issued_at: 1000,
            expires_at: 2000,
            allowed_providers: vec!["anthropic".to_string(), "fake".to_string()],
            lanes: vec![
                ModelLaneConfig {
                    lane: "coding".to_string(),
                    provider: "anthropic".to_string(),
                    model_sku: "claude-3-7-sonnet".to_string(),
                },
            ],
            required_sandbox_guarantee: "strong".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };

        policy.sign(&key).expect("signing succeeds");
        assert!(policy.verify_signature().expect("verification succeeds"));

        assert!(!policy.is_expired(1500));
        assert!(policy.is_expired(2500));

        // Tamper detection
        policy.required_sandbox_guarantee = "read_only".to_string();
        assert!(!policy.verify_signature().unwrap_or(false));
    }
}