//! Execution policy engine, exact-action digests, capability checks, and signed model policies.

pub mod mandate;

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

    /// Validates schema version, optional expected public key, Ed25519 signature,
    /// and expiration timestamp, returning the canonical SHA-256 policy digest.
    pub fn verify_for_use(
        &self,
        now_unix_secs: u64,
        expected_public_key_hex: Option<&str>,
    ) -> Result<String, String> {
        if self.schema_version != MODEL_POLICY_SCHEMA_V1 {
            return Err(format!(
                "unsupported model policy schema '{}': expected '{}'",
                self.schema_version, MODEL_POLICY_SCHEMA_V1
            ));
        }
        if self.policy_id.trim().is_empty() {
            return Err("model policy missing non-empty policy_id".to_string());
        }
        if !VALID_SANDBOX_GUARANTEES.contains(&self.required_sandbox_guarantee.as_str()) {
            return Err(format!(
                "invalid required_sandbox_guarantee '{}': expected one of {}",
                self.required_sandbox_guarantee,
                VALID_SANDBOX_GUARANTEES.join(", ")
            ));
        }
        if let Some(expected_pk) = expected_public_key_hex {
            if !self.public_key.eq_ignore_ascii_case(expected_pk.trim()) {
                return Err("model policy was signed by a different public key than expected".to_string());
            }
        }
        let sig_ok = self
            .verify_signature()
            .map_err(|e| format!("model policy signature check failed: {e}"))?;
        if !sig_ok {
            return Err("model policy Ed25519 signature is invalid or payload was tampered".to_string());
        }
        if self.is_expired(now_unix_secs) {
            return Err(format!(
                "model policy '{}' expired at {} (current unix time {})",
                self.policy_id, self.expires_at, now_unix_secs
            ));
        }
        self.policy_digest()
            .map_err(|e| format!("failed to compute model policy digest: {e}"))
    }

    /// Enforces a verified signed model policy against a concrete provider,
    /// model SKU, and sandbox guarantee, failing closed on any violation.
    /// Returns the canonical SHA-256 policy digest on success.
    pub fn enforce(
        &self,
        now_unix_secs: u64,
        expected_public_key_hex: Option<&str>,
        provider: &str,
        model_sku: &str,
        sandbox_guarantee: &str,
    ) -> Result<String, String> {
        let digest = self.verify_for_use(now_unix_secs, expected_public_key_hex)?;

        if !self.allowed_providers.is_empty()
            && !self
                .allowed_providers
                .iter()
                .any(|p| p.eq_ignore_ascii_case(provider))
        {
            return Err(format!(
                "provider '{provider}' is not permitted by model policy '{}' (allowed: {})",
                self.policy_id,
                self.allowed_providers.join(", ")
            ));
        }

        if !self.lanes.is_empty() {
            let lane_ok = self.lanes.iter().any(|l| {
                l.provider.eq_ignore_ascii_case(provider)
                    && (l.model_sku == model_sku || l.lane == model_sku)
            });
            if !lane_ok {
                return Err(format!(
                    "model '{model_sku}' on provider '{provider}' is not in any lane allowed by model policy '{}'",
                    self.policy_id
                ));
            }
        }

        if !sandbox_satisfies_requirement(sandbox_guarantee, &self.required_sandbox_guarantee) {
            return Err(format!(
                "sandbox guarantee '{sandbox_guarantee}' does not satisfy model policy '{}' requirement '{}'",
                self.policy_id, self.required_sandbox_guarantee
            ));
        }

        Ok(digest)
    }
}

pub const MODEL_POLICY_SCHEMA_V1: &str = "vitna-model-policy-v1";
pub const VALID_SANDBOX_GUARANTEES: &[&str] = &["read_only", "guarded", "strong", "full_access"];

/// Returns true iff `actual` meets or exceeds the policy's `required` sandbox guarantee.
pub fn sandbox_satisfies_requirement(actual: &str, required: &str) -> bool {
    if !VALID_SANDBOX_GUARANTEES.contains(&actual)
        || !VALID_SANDBOX_GUARANTEES.contains(&required)
    {
        return false;
    }
    match required {
        "full_access" => true,
        "guarded" => matches!(actual, "guarded" | "read_only" | "strong"),
        "read_only" => actual == "read_only",
        "strong" => actual == "strong",
        _ => false,
    }
}

pub fn generate_policy_signing_key() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_policy(key: &SigningKey) -> SignedModelPolicy {
        let mut policy = SignedModelPolicy {
            schema_version: MODEL_POLICY_SCHEMA_V1.to_string(),
            policy_id: "test-policy-01".to_string(),
            issued_at: 1000,
            expires_at: 2000,
            allowed_providers: vec!["anthropic".to_string(), "vitna-runtime".to_string()],
            lanes: vec![
                ModelLaneConfig {
                    lane: "coding".to_string(),
                    provider: "anthropic".to_string(),
                    model_sku: "claude-sonnet-4-5".to_string(),
                },
                ModelLaneConfig {
                    lane: "vitna/code".to_string(),
                    provider: "vitna-runtime".to_string(),
                    model_sku: "vitna/code".to_string(),
                },
            ],
            required_sandbox_guarantee: "guarded".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };
        policy.sign(key).expect("signing succeeds");
        policy
    }

    #[test]
    fn test_signed_model_policy_lifecycle() {
        let key = generate_policy_signing_key();
        let mut policy = sample_policy(&key);

        assert!(policy.verify_signature().expect("verification succeeds"));
        assert!(!policy.is_expired(1500));
        assert!(policy.is_expired(2500));

        // Tamper detection
        policy.required_sandbox_guarantee = "read_only".to_string();
        assert!(!policy.verify_signature().unwrap_or(false));
    }

    #[test]
    fn enforce_allows_matching_route_and_refuses_violations_fail_closed() {
        let key = generate_policy_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());
        let policy = sample_policy(&key);

        // Matching provider, lane, and sandbox succeeds and returns 64-char digest.
        let digest = policy
            .enforce(1500, Some(&pub_hex), "anthropic", "claude-sonnet-4-5", "guarded")
            .expect("matching route allowed");
        assert_eq!(digest.len(), 64);

        // Stronger sandbox (`strong`) satisfies `guarded` requirement.
        assert!(policy
            .enforce(1500, Some(&pub_hex), "anthropic", "claude-sonnet-4-5", "strong")
            .is_ok());

        // Downgrading to `full_access` under `guarded` policy is refused.
        assert!(policy
            .enforce(1500, Some(&pub_hex), "anthropic", "claude-sonnet-4-5", "full_access")
            .is_err());

        // Disallowed provider is refused.
        assert!(policy
            .enforce(1500, Some(&pub_hex), "openai", "gpt-5", "guarded")
            .is_err());

        // Disallowed model SKU on allowed provider is refused.
        assert!(policy
            .enforce(1500, Some(&pub_hex), "anthropic", "unapproved-model", "guarded")
            .is_err());

        // Expired policy is refused.
        assert!(policy
            .enforce(2001, Some(&pub_hex), "anthropic", "claude-sonnet-4-5", "guarded")
            .is_err());

        // Wrong expected public key is refused.
        let other_key = generate_policy_signing_key();
        let other_hex = hex::encode(other_key.verifying_key().to_bytes());
        assert!(policy
            .enforce(1500, Some(&other_hex), "anthropic", "claude-sonnet-4-5", "guarded")
            .is_err());

        // `strong` requirement refuses `guarded` and `read_only`.
        assert!(sandbox_satisfies_requirement("strong", "strong"));
        assert!(!sandbox_satisfies_requirement("guarded", "strong"));
        assert!(!sandbox_satisfies_requirement("read_only", "strong"));
    }
}