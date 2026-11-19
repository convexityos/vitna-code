//! Cross-product offline provenance auditing for Vitna Code receipts,
//! Vitna Anchor checkpoints, and Convexity API DataEnvelopes.

use crate::{ReceiptVerifier, VerificationReport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use vitna_receipts::VitnaRunReceiptV1;

/// Audit verdict for a local `vitna-anchor` checkpoint cross-checked against a receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointAudit {
    pub path: String,
    pub local_sha256: Option<String>,
    pub receipt_policy_digest: Option<String>,
    pub matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Audit verdict for a Convexity API `DataEnvelope` JSON record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeAudit {
    pub source_file: String,
    pub is_valid: bool,
    pub sync_status: String,
    pub data_as_of: Option<String>,
    pub source: Option<String>,
    pub data_is_null: bool,
    pub citation: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Options for [`verify_provenance_bundle`].
#[derive(Debug, Clone, Default)]
pub struct ProvenanceAuditOptions {
    pub receipt_path: PathBuf,
    pub public_key_hex: Option<String>,
    pub require_signature: bool,
    pub checkpoint_path: Option<PathBuf>,
    pub envelope_paths: Vec<PathBuf>,
    pub strict_freshness: bool,
}

/// Machine-readable report combining receipt, checkpoint, and DataEnvelope verdicts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceAuditReport {
    pub overall_valid: bool,
    pub canonical_sha256: String,
    pub receipt: VerificationReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointAudit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub envelopes: Vec<EnvelopeAudit>,
    pub errors: Vec<String>,
}

/// Computes a deterministic lowercase 64-char SHA-256 hex digest for a local
/// checkpoint file or model directory, matching `vitna-anchor`'s
/// `vitna_checkpoint_sha256` and `vitna-providers`'s `compute_checkpoint_digest`
/// byte for byte.
pub fn compute_checkpoint_digest(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path)
        .map_err(|e| format!("cannot stat checkpoint '{}': {}", path.display(), e))?;
    if meta.is_file() {
        return hash_file_sha256(path);
    }
    if !meta.is_dir() {
        return Err(format!(
            "checkpoint '{}' is neither a file nor a directory",
            path.display()
        ));
    }

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|e| format!("cannot read checkpoint directory '{}': {}", path.display(), e))?
    {
        let entry = entry.map_err(|e| format!("dir entry error: {}", e))?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_checkpoint_artifact(&name) {
            entries.push((name, p));
        }
    }
    if entries.is_empty() {
        return Err(format!(
            "checkpoint directory '{}' contains no recognized model artifacts",
            path.display()
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest_hasher = Sha256::new();
    for (name, file_path) in entries {
        let file_hex = hash_file_sha256(&file_path)?;
        manifest_hasher.update(name.as_bytes());
        manifest_hasher.update(b":");
        manifest_hasher.update(file_hex.as_bytes());
        manifest_hasher.update(b"\n");
    }
    Ok(hex::encode(manifest_hasher.finalize()))
}

fn is_checkpoint_artifact(name: &str) -> bool {
    name.ends_with(".gguf")
        || name.ends_with(".safetensors")
        || name == "config.json"
        || name == "tokenizer.json"
        || name == "tokenizer_config.json"
}

fn hash_file_sha256(path: &Path) -> Result<String, String> {
    let mut f = File::open(path)
        .map_err(|e| format!("cannot open '{}': {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("error reading '{}': {}", path.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Cross-checks a local checkpoint file or directory against `receipt.model_selection.policy_digest`.
pub fn audit_checkpoint(receipt: &VitnaRunReceiptV1, checkpoint_path: &Path) -> CheckpointAudit {
    let path_str = checkpoint_path.to_string_lossy().into_owned();
    let receipt_digest = receipt.model_selection.policy_digest.clone();

    let local_sha256 = match compute_checkpoint_digest(checkpoint_path) {
        Ok(h) => h,
        Err(err) => {
            return CheckpointAudit {
                path: path_str,
                local_sha256: None,
                receipt_policy_digest: receipt_digest,
                matched: false,
                error: Some(err),
            };
        }
    };

    match &receipt_digest {
        Some(expected) if expected.trim().eq_ignore_ascii_case(&local_sha256) => CheckpointAudit {
            path: path_str,
            local_sha256: Some(local_sha256),
            receipt_policy_digest: receipt_digest,
            matched: true,
            error: None,
        },
        Some(expected) => CheckpointAudit {
            path: path_str,
            local_sha256: Some(local_sha256.clone()),
            receipt_policy_digest: receipt_digest.clone(),
            matched: false,
            error: Some(format!(
                "checkpoint SHA-256 mismatch: local '{}' != receipt policy_digest '{}'",
                local_sha256, expected
            )),
        },
        None => CheckpointAudit {
            path: path_str,
            local_sha256: Some(local_sha256),
            receipt_policy_digest: None,
            matched: false,
            error: Some("receipt.model_selection.policy_digest is absent; cannot verify checkpoint".to_string()),
        },
    }
}

/// Verifies a Convexity API `DataEnvelope` JSON string.
pub fn audit_envelope_json(
    source_file: impl Into<String>,
    content: &str,
    strict_freshness: bool,
) -> EnvelopeAudit {
    let source_file = source_file.into();
    let val: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return EnvelopeAudit {
                source_file,
                is_valid: false,
                sync_status: "invalid".to_string(),
                data_as_of: None,
                source: None,
                data_is_null: true,
                citation: "invalid JSON".to_string(),
                warnings: Vec::new(),
                errors: vec![format!("invalid DataEnvelope JSON: {}", e)],
            };
        }
    };

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let Some(obj) = val.as_object() else {
        return EnvelopeAudit {
            source_file,
            is_valid: false,
            sync_status: "invalid".to_string(),
            data_as_of: None,
            source: None,
            data_is_null: true,
            citation: "invalid envelope".to_string(),
            warnings,
            errors: vec!["DataEnvelope must be a JSON object".to_string()],
        };
    };

    let data_is_null = obj.get("data").map(|d| d.is_null()).unwrap_or(true);
    if !obj.contains_key("data") {
        errors.push("DataEnvelope missing 'data' key".to_string());
    }

    let meta = obj.get("meta").and_then(|m| m.as_object());
    if meta.is_none() {
        errors.push("DataEnvelope missing 'meta' object".to_string());
    }

    let sync_status = meta
        .and_then(|m| m.get("sync_status"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let data_as_of = meta
        .and_then(|m| m.get("data_as_of"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let source = meta
        .and_then(|m| m.get("source"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if sync_status == "failed" {
        errors.push("upstream sync_status is 'failed'".to_string());
    } else if sync_status == "stale" {
        if strict_freshness {
            errors.push("upstream sync_status is 'stale' (--strict-freshness enabled)".to_string());
        } else {
            warnings.push("upstream sync_status is 'stale'".to_string());
        }
    }

    if data_is_null {
        warnings.push("data is null (explicit absence, not an error)".to_string());
    }

    let time_part = match &data_as_of {
        Some(ts) => format!("as of {}", ts),
        None => "no data timestamp".to_string(),
    };
    let citation = match &source {
        Some(src) => format!("{} [{}, source={}]", time_part, sync_status, src),
        None => format!("{} [{}]", time_part, sync_status),
    };

    EnvelopeAudit {
        source_file,
        is_valid: errors.is_empty(),
        sync_status,
        data_as_of,
        source,
        data_is_null,
        citation,
        warnings,
        errors,
    }
}

/// Runs a unified offline provenance audit across a Vitna receipt, an optional
/// local `vitna-anchor` checkpoint, and optional Convexity `DataEnvelope` files.
pub fn verify_provenance_bundle(
    opts: &ProvenanceAuditOptions,
) -> Result<ProvenanceAuditReport, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(&opts.receipt_path)?;
    let receipt: VitnaRunReceiptV1 = serde_json::from_str(&raw)?;
    let canonical_bytes = receipt.to_canonical_bytes()?;
    let canonical_sha256 = hex::encode(Sha256::digest(&canonical_bytes));

    let receipt_report =
        ReceiptVerifier::verify_receipt(&receipt, opts.public_key_hex.as_deref())?;

    let mut errors = receipt_report.errors.clone();

    if opts.require_signature && !receipt_report.signature_verified {
        errors.push(
            "device signature was not verified (--require-signature requires a valid --public-key)"
                .to_string(),
        );
    }

    let checkpoint = opts.checkpoint_path.as_ref().map(|p| {
        let audit = audit_checkpoint(&receipt, p);
        if !audit.matched {
            if let Some(ref e) = audit.error {
                errors.push(e.clone());
            }
        }
        audit
    });

    let mut envelopes = Vec::with_capacity(opts.envelope_paths.len());
    for env_path in &opts.envelope_paths {
        let label = env_path.to_string_lossy().into_owned();
        let audit = match fs::read_to_string(env_path) {
            Ok(content) => audit_envelope_json(&label, &content, opts.strict_freshness),
            Err(e) => EnvelopeAudit {
                source_file: label,
                is_valid: false,
                sync_status: "io_error".to_string(),
                data_as_of: None,
                source: None,
                data_is_null: true,
                citation: "unreadable".to_string(),
                warnings: Vec::new(),
                errors: vec![format!("cannot read envelope file: {}", e)],
            },
        };
        if !audit.is_valid {
            for e in &audit.errors {
                errors.push(format!("{}: {}", audit.source_file, e));
            }
        }
        envelopes.push(audit);
    }

    Ok(ProvenanceAuditReport {
        overall_valid: errors.is_empty(),
        canonical_sha256,
        receipt: receipt_report,
        checkpoint,
        envelopes,
        errors,
    })
}
