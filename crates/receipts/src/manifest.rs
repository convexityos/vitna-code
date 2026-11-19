//! Signed distribution release manifests (`schemas/vitna-release-manifest-v1.json`).
//!
//! Provides deterministic RFC 8785 canonical JSON signing, offline Ed25519
//! publisher signature verification, and on-disk archive SHA-256 + byte-size
//! verification so release claims are cryptographically auditable rather than
//! asserted by hand.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use crate::VitnaRunReceiptV1;

/// Declared schema identifier matching `schemas/vitna-release-manifest-v1.json`.
pub const RELEASE_MANIFEST_SCHEMA_V1: &str = "vitna-release-manifest-v1";

/// Supported Tier 1 target triples declared in `schemas/vitna-release-manifest-v1.json`.
pub const SUPPORTED_RELEASE_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

/// Supported release channels declared in `schemas/vitna-release-manifest-v1.json`.
pub const SUPPORTED_RELEASE_CHANNELS: &[&str] = &["stable", "beta", "nightly"];

/// One packaged binary archive entry inside a release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub target_triple: String,
    pub filename: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub min_os_version: String,
}

impl ReleaseAsset {
    /// Builds a [`ReleaseAsset`] by reading an actual archive file on disk and
    /// hashing its exact bytes with SHA-256.
    pub fn from_file(
        path: &Path,
        target_triple: impl Into<String>,
        min_os_version: impl Into<String>,
    ) -> Result<Self, String> {
        let target_triple = target_triple.into();
        if !SUPPORTED_RELEASE_TARGETS.contains(&target_triple.as_str()) {
            return Err(format!(
                "unsupported target_triple '{target_triple}' (expected one of: {})",
                SUPPORTED_RELEASE_TARGETS.join(", ")
            ));
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("asset path '{}' has no valid UTF-8 filename", path.display()))?
            .to_string();
        validate_asset_filename(&filename)?;

        let bytes = fs::read(path)
            .map_err(|e| format!("could not read release asset '{}': {e}", path.display()))?;
        if bytes.is_empty() {
            return Err(format!(
                "release asset '{}' is empty (0 bytes)",
                path.display()
            ));
        }

        let sha256 = hex::encode(Sha256::digest(&bytes));
        Ok(Self {
            target_triple,
            filename,
            sha256,
            size_bytes: bytes.len() as u64,
            min_os_version: min_os_version.into(),
        })
    }
}

/// Cryptographically signed distribution manifest matching
/// `schemas/vitna-release-manifest-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VitnaReleaseManifestV1 {
    pub schema_version: String,
    pub version: String,
    pub channel: String,
    pub release_date: String,
    pub release_notes_url: String,
    pub assets: Vec<ReleaseAsset>,
    #[serde(default)]
    pub publisher_public_key: String,
    #[serde(default)]
    pub publisher_signature: String,
}

/// Detailed result of offline release manifest verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestVerificationReport {
    pub is_valid: bool,
    pub schema_valid: bool,
    pub signature_verified: bool,
    pub publisher_key_matched: Option<bool>,
    pub assets_checked: usize,
    pub errors: Vec<String>,
}

impl VitnaReleaseManifestV1 {
    /// Computes RFC 8785 canonical JSON bytes over all manifest fields except
    /// `publisher_signature` (which signs those canonical bytes).
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut clone = self.clone();
        clone.publisher_signature.clear();
        let value = serde_json::to_value(&clone)?;
        Ok(VitnaRunReceiptV1::canonicalize_value(&value).into_bytes())
    }

    /// Signs this manifest in place with `signing_key`, setting both
    /// `publisher_public_key` and `publisher_signature`.
    pub fn sign(&mut self, signing_key: &SigningKey) -> Result<(), String> {
        self.publisher_public_key = hex::encode(signing_key.verifying_key().to_bytes());
        self.publisher_signature.clear();
        let bytes = self
            .to_canonical_bytes()
            .map_err(|e| format!("canonicalization failed: {e}"))?;
        let sig = signing_key.sign(&bytes);
        self.publisher_signature = hex::encode(sig.to_bytes());
        Ok(())
    }

    /// Performs full offline structural and cryptographic Ed25519 verification
    /// of this manifest, optionally pinning `expected_publisher_key_hex`.
    pub fn verify(&self, expected_publisher_key_hex: Option<&str>) -> ManifestVerificationReport {
        let mut errors = Vec::new();
        let mut schema_valid = true;

        if self.schema_version != RELEASE_MANIFEST_SCHEMA_V1 {
            schema_valid = false;
            errors.push(format!(
                "unsupported schema_version '{}' (expected '{RELEASE_MANIFEST_SCHEMA_V1}')",
                self.schema_version
            ));
        }
        if !is_valid_semver(&self.version) {
            schema_valid = false;
            errors.push(format!("invalid release version '{}'", self.version));
        }
        if !SUPPORTED_RELEASE_CHANNELS.contains(&self.channel.as_str()) {
            schema_valid = false;
            errors.push(format!(
                "invalid channel '{}' (expected one of: {})",
                self.channel,
                SUPPORTED_RELEASE_CHANNELS.join(", ")
            ));
        }
        if self.release_date.trim().is_empty() {
            schema_valid = false;
            errors.push("release_date must be non-empty".to_string());
        }
        if self.assets.is_empty() {
            schema_valid = false;
            errors.push("assets list must contain at least one release asset".to_string());
        }

        for asset in &self.assets {
            if !SUPPORTED_RELEASE_TARGETS.contains(&asset.target_triple.as_str()) {
                schema_valid = false;
                errors.push(format!(
                    "asset '{}' has unsupported target_triple '{}'",
                    asset.filename, asset.target_triple
                ));
            }
            if let Err(reason) = validate_asset_filename(&asset.filename) {
                schema_valid = false;
                errors.push(reason);
            }
            if !is_hex_len(&asset.sha256, 64) {
                schema_valid = false;
                errors.push(format!(
                    "asset '{}' sha256 must be 64 lowercase hex characters",
                    asset.filename
                ));
            }
            if asset.size_bytes == 0 {
                schema_valid = false;
                errors.push(format!(
                    "asset '{}' size_bytes must be >= 1",
                    asset.filename
                ));
            }
        }

        let publisher_key_matched = expected_publisher_key_hex.map(|expected| {
            let matched = self.publisher_public_key.eq_ignore_ascii_case(expected.trim());
            if !matched {
                errors.push(format!(
                    "publisher_public_key mismatch: manifest carries '{}', expected '{}'",
                    self.publisher_public_key,
                    expected.trim()
                ));
            }
            matched
        });

        let signature_verified = match self.verify_signature() {
            Ok(()) => true,
            Err(reason) => {
                errors.push(reason);
                false
            }
        };

        ManifestVerificationReport {
            is_valid: schema_valid
                && signature_verified
                && publisher_key_matched.unwrap_or(true)
                && errors.is_empty(),
            schema_valid,
            signature_verified,
            publisher_key_matched,
            assets_checked: self.assets.len(),
            errors,
        }
    }

    fn verify_signature(&self) -> Result<(), String> {
        let key_hex = self.publisher_public_key.trim();
        let sig_hex = self.publisher_signature.trim();
        if !is_hex_len(key_hex, 64) {
            return Err(
                "publisher_public_key must be a 64-character hex Ed25519 public key".to_string(),
            );
        }
        if !is_hex_len(sig_hex, 128) {
            return Err(
                "publisher_signature must be a 128-character hex Ed25519 signature".to_string(),
            );
        }

        let key_bytes = hex::decode(key_hex)
            .map_err(|e| format!("invalid publisher_public_key hex: {e}"))?;
        let key_arr: [u8; 32] = key_bytes
            .try_into()
            .map_err(|_| "publisher_public_key must decode to 32 bytes".to_string())?;
        let verifying_key = VerifyingKey::from_bytes(&key_arr)
            .map_err(|e| format!("invalid Ed25519 publisher_public_key: {e}"))?;

        let sig_bytes = hex::decode(sig_hex)
            .map_err(|e| format!("invalid publisher_signature hex: {e}"))?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| "publisher_signature must decode to 64 bytes".to_string())?;
        let signature = Signature::from_bytes(&sig_arr);

        let canonical = self
            .to_canonical_bytes()
            .map_err(|e| format!("failed to canonicalize manifest: {e}"))?;
        verifying_key
            .verify_strict(&canonical, &signature)
            .map_err(|_| {
                "Ed25519 publisher_signature verification failed against canonical manifest payload"
                    .to_string()
            })
    }

    /// Verifies that every asset listed in the manifest exists in `assets_dir`
    /// with exact matching byte length and SHA-256 digest.
    pub fn verify_assets_in_dir(&self, assets_dir: &Path) -> Result<Vec<String>, Vec<String>> {
        let mut verified = Vec::new();
        let mut errors = Vec::new();

        for asset in &self.assets {
            if let Err(reason) = validate_asset_filename(&asset.filename) {
                errors.push(reason);
                continue;
            }
            let file_path = assets_dir.join(&asset.filename);
            let bytes = match fs::read(&file_path) {
                Ok(b) => b,
                Err(e) => {
                    errors.push(format!(
                        "missing or unreadable asset '{}': {e}",
                        file_path.display()
                    ));
                    continue;
                }
            };
            if bytes.len() as u64 != asset.size_bytes {
                errors.push(format!(
                    "asset '{}' size mismatch: manifest declares {} bytes, found {} bytes",
                    asset.filename,
                    asset.size_bytes,
                    bytes.len()
                ));
                continue;
            }
            let actual_sha = hex::encode(Sha256::digest(&bytes));
            if !actual_sha.eq_ignore_ascii_case(&asset.sha256) {
                errors.push(format!(
                    "asset '{}' SHA-256 mismatch: manifest declares {}, disk has {}",
                    asset.filename, asset.sha256, actual_sha
                ));
                continue;
            }
            verified.push(asset.filename.clone());
        }

        if errors.is_empty() {
            Ok(verified)
        } else {
            Err(errors)
        }
    }
}

fn validate_asset_filename(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(format!(
            "unsafe asset filename '{name}': must be a plain basename with no path separators"
        ));
    }
    Ok(())
}

fn is_hex_len(s: &str, expected_len: usize) -> bool {
    s.len() == expected_len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_valid_semver(version: &str) -> bool {
    let base = version.split('-').next().unwrap_or("");
    let parts: Vec<&str> = base.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_signing_key;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("vitna_manifest_{tag}_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn release_manifest_sign_verify_and_asset_check_round_trip() {
        let dir = temp_dir("roundtrip");
        let archive = dir.join("vitna-code-v0.2.0-x86_64-unknown-linux-gnu.tar.gz");
        fs::write(&archive, b"deterministic-archive-bytes-v0.2.0").expect("write archive");

        let asset = ReleaseAsset::from_file(
            &archive,
            "x86_64-unknown-linux-gnu",
            "Linux kernel 5.15+, glibc 2.31+",
        )
        .expect("asset from file");

        let mut manifest = VitnaReleaseManifestV1 {
            schema_version: RELEASE_MANIFEST_SCHEMA_V1.to_string(),
            version: "0.2.0-beta.1".to_string(),
            channel: "beta".to_string(),
            release_date: "2026-11-19T17:00:00Z".to_string(),
            release_notes_url: "https://github.com/convexityos/vitna-code/releases/tag/v0.2.0-beta.1"
                .to_string(),
            assets: vec![asset],
            publisher_public_key: String::new(),
            publisher_signature: String::new(),
        };

        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());
        manifest.sign(&key).expect("sign manifest");

        let report = manifest.verify(Some(&pub_hex));
        assert!(report.is_valid, "errors: {:?}", report.errors);
        assert!(report.signature_verified);
        assert_eq!(report.publisher_key_matched, Some(true));

        let checked = manifest
            .verify_assets_in_dir(&dir)
            .expect("disk asset hashes match");
        assert_eq!(
            checked,
            vec!["vitna-code-v0.2.0-x86_64-unknown-linux-gnu.tar.gz".to_string()]
        );

        // Wrong expected publisher key fails closed.
        let other_key = generate_signing_key();
        let other_hex = hex::encode(other_key.verifying_key().to_bytes());
        let wrong_key = manifest.verify(Some(&other_hex));
        assert!(!wrong_key.is_valid);
        assert_eq!(wrong_key.publisher_key_matched, Some(false));

        // Tampering with any signed field invalidates the Ed25519 signature.
        let mut tampered = manifest.clone();
        tampered.assets[0].size_bytes += 1;
        let bad_sig = tampered.verify(Some(&pub_hex));
        assert!(!bad_sig.is_valid);
        assert!(!bad_sig.signature_verified);

        // Tampering with the archive on disk fails verify_assets_in_dir.
        fs::write(&archive, b"tampered-bytes-different-length-and-hash").unwrap();
        let disk_err = manifest.verify_assets_in_dir(&dir).unwrap_err();
        assert!(!disk_err.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn historical_unbuilt_v1_manifest_fails_cryptographic_verification() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("releases")
            .join("v1.0.0")
            .join("manifest.json");
        let raw = fs::read_to_string(&path).expect("read historical v1.0.0 manifest");
        let manifest: VitnaReleaseManifestV1 =
            serde_json::from_str(&raw).expect("parse historical manifest");

        // As documented in IMPLEMENTATION_STATUS.md, the historical v1.0.0
        // manifest was never signed with a real Ed25519 signature over real
        // canonical bytes. Our offline verifier must reject it honestly.
        let report = manifest.verify(None);
        assert!(
            !report.signature_verified,
            "historical unbuilt manifest must not verify as authentically signed"
        );
        assert!(!report.is_valid);
    }
}
