use std::fs;
use std::path::PathBuf;
use vitna_policy::SignedModelPolicy;

fn get_fixtures_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // manifest_dir is crates/receipts, so fixtures is ../../fixtures
    manifest_dir.join("../../fixtures/policies")
}

#[test]
fn test_valid_model_policy_fixture() {
    let fixture_path = get_fixtures_dir().join("valid-model-policy.json");
    let content = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture at {:?}: {}", fixture_path, e));

    let policy: SignedModelPolicy = serde_json::from_str(&content).expect("Valid JSON schema");
    let is_valid = policy.verify_signature().expect("Signature verification process");
    assert!(is_valid, "Valid policy fixture signature must verify successfully");

    // Check expiration against an active 2026 timestamp
    let current_time_2026 = 1773650000u64;
    assert!(!policy.is_expired(current_time_2026), "Policy should not be expired in 2026");

    // Check digest computation
    let digest = policy.policy_digest().expect("Digest computation");
    assert_eq!(digest.len(), 64, "SHA-256 digest must be 64 hex characters");
}

#[test]
fn test_tampered_model_policy_fixture() {
    let fixture_path = get_fixtures_dir().join("tampered-model-policy.json");
    let content = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture at {:?}: {}", fixture_path, e));

    let policy: SignedModelPolicy = serde_json::from_str(&content).expect("Valid JSON schema");
    let is_valid = policy.verify_signature().unwrap_or(false);
    assert!(!is_valid, "Tampered policy fixture must fail cryptographic verification");
}

#[test]
fn test_expired_model_policy_fixture() {
    let fixture_path = get_fixtures_dir().join("expired-model-policy.json");
    let content = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture at {:?}: {}", fixture_path, e));

    let policy: SignedModelPolicy = serde_json::from_str(&content).expect("Valid JSON schema");
    let is_valid = policy.verify_signature().expect("Signature verification process");
    assert!(is_valid, "Expired policy signature itself remains cryptographically authentic");

    // But when evaluated against 2026 timestamp, it is expired
    let current_time_2026 = 1773650000u64;
    assert!(policy.is_expired(current_time_2026), "Expired policy must be rejected by expiration check");
}
