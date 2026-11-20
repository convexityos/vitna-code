//! Side-by-side comparison of two `VitnaRunReceiptV1` artifacts.
//!
//! Shared by `vitna-app-server` (`POST /api/v1/receipt/diff`), `vitna-tui`
//! (`/diff`), and `vitna-cli` so every surface reports identical model changes,
//! isolation regressions, file deltas, evidence grades, new egress domains, and
//! `am/1` mandate spend deltas.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use vitna_receipts::{ModelSelectionRecord, VitnaRunReceiptV1};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDiff {
    pub changed: bool,
    pub a: ModelSelectionRecord,
    pub b: ModelSelectionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolationDiff {
    pub changed: bool,
    pub regression: bool,
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionDiff {
    pub changed: bool,
    pub a: String,
    pub b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
    pub postimage_changed: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceDiff {
    pub count_a: usize,
    pub count_b: usize,
    pub highest_grade_a: String,
    pub highest_grade_b: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressDiff {
    pub count_a: usize,
    pub count_b: usize,
    pub new_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateDiff {
    pub root_a: Option<String>,
    pub root_b: Option<String>,
    pub spent_a: Option<String>,
    pub spent_b: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptDiff {
    pub run_a: String,
    pub run_b: String,
    pub model: ModelDiff,
    pub isolation: IsolationDiff,
    pub completion: CompletionDiff,
    pub files: FileDiff,
    pub evidence: EvidenceDiff,
    pub egress: EgressDiff,
    pub mandate: MandateDiff,
}

fn isolation_strength_rank(label: &str) -> Option<u8> {
    match label {
        "full_access" => Some(0),
        "guarded" => Some(1),
        "read_only" => Some(2),
        "strong" => Some(3),
        _ => None,
    }
}

pub fn highest_evidence_grade(receipt: &VitnaRunReceiptV1) -> &'static str {
    const LADDER: &[&str] = &[
        "model_reported",
        "broker_observed",
        "sandbox_captured",
        "independently_reproduced",
        "remote_or_hardware_attested",
    ];
    let mut best: Option<usize> = None;
    for item in &receipt.evidence_items {
        if let Some(idx) = LADDER.iter().position(|g| *g == item.grade) {
            best = Some(best.map_or(idx, |b| b.max(idx)));
        }
    }
    best.map(|idx| LADDER[idx]).unwrap_or("none")
}

/// Computes a deterministic diff between `receipt_a` (baseline) and `receipt_b` (candidate).
pub fn diff_receipts(receipt_a: &VitnaRunReceiptV1, receipt_b: &VitnaRunReceiptV1) -> ReceiptDiff {
    let files_a: BTreeMap<&str, &str> = receipt_a
        .changeset
        .files_modified
        .iter()
        .map(|f| (f.path.as_str(), f.postimage_hash.as_str()))
        .collect();
    let files_b: BTreeMap<&str, &str> = receipt_b
        .changeset
        .files_modified
        .iter()
        .map(|f| (f.path.as_str(), f.postimage_hash.as_str()))
        .collect();

    let mut only_in_a = Vec::new();
    let mut only_in_b = Vec::new();
    let mut postimage_changed = Vec::new();
    let mut unchanged = Vec::new();

    for (path, hash_a) in &files_a {
        match files_b.get(path) {
            None => only_in_a.push((*path).to_string()),
            Some(hash_b) if hash_b != hash_a => postimage_changed.push((*path).to_string()),
            Some(_) => unchanged.push((*path).to_string()),
        }
    }
    for path in files_b.keys() {
        if !files_a.contains_key(path) {
            only_in_b.push((*path).to_string());
        }
    }

    let model_changed = receipt_a.model_selection.provider != receipt_b.model_selection.provider
        || receipt_a.model_selection.model_sku != receipt_b.model_selection.model_sku
        || receipt_a.model_selection.policy_digest != receipt_b.model_selection.policy_digest;

    let isolation_changed = receipt_a.isolation_label != receipt_b.isolation_label;
    let isolation_regression = match (
        isolation_strength_rank(&receipt_a.isolation_label),
        isolation_strength_rank(&receipt_b.isolation_label),
    ) {
        (Some(ra), Some(rb)) => rb < ra,
        _ => false,
    };

    let domains_a: BTreeSet<&str> = receipt_a
        .egress_calls
        .iter()
        .map(|e| e.target_domain.as_str())
        .collect();
    let new_domains: Vec<String> = receipt_b
        .egress_calls
        .iter()
        .map(|e| e.target_domain.as_str())
        .filter(|d| !domains_a.contains(d))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect();

    let spent_a = receipt_a
        .mandate_provenance
        .as_ref()
        .map(|m| format!("{} {}", m.total_spent, m.currency));
    let spent_b = receipt_b
        .mandate_provenance
        .as_ref()
        .map(|m| format!("{} {}", m.total_spent, m.currency));

    ReceiptDiff {
        run_a: receipt_a.run_id.clone(),
        run_b: receipt_b.run_id.clone(),
        model: ModelDiff {
            changed: model_changed,
            a: receipt_a.model_selection.clone(),
            b: receipt_b.model_selection.clone(),
        },
        isolation: IsolationDiff {
            changed: isolation_changed,
            regression: isolation_regression,
            a: receipt_a.isolation_label.clone(),
            b: receipt_b.isolation_label.clone(),
        },
        completion: CompletionDiff {
            changed: receipt_a.completion_state != receipt_b.completion_state,
            a: receipt_a.completion_state.clone(),
            b: receipt_b.completion_state.clone(),
        },
        files: FileDiff {
            only_in_a,
            only_in_b,
            postimage_changed,
            unchanged,
        },
        evidence: EvidenceDiff {
            count_a: receipt_a.evidence_items.len(),
            count_b: receipt_b.evidence_items.len(),
            highest_grade_a: highest_evidence_grade(receipt_a).to_string(),
            highest_grade_b: highest_evidence_grade(receipt_b).to_string(),
        },
        egress: EgressDiff {
            count_a: receipt_a.egress_calls.len(),
            count_b: receipt_b.egress_calls.len(),
            new_domains,
        },
        mandate: MandateDiff {
            root_a: receipt_a
                .mandate_provenance
                .as_ref()
                .map(|m| m.root_jti.clone()),
            root_b: receipt_b
                .mandate_provenance
                .as_ref()
                .map(|m| m.root_jti.clone()),
            spent_a,
            spent_b,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitna_receipts::{
        ChangeSetRecord, EgressCallRecord, EvidenceItemRecord, FileModificationRecord,
        MandateProvenanceRecord,
    };

    fn sample_receipt(run_id: &str, isolation: &str) -> VitnaRunReceiptV1 {
        VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: run_id.to_string(),
            session_id: "sess-1".to_string(),
            workspace_fingerprint: "a".repeat(64),
            base_commit_sha: "b".repeat(40),
            model_selection: ModelSelectionRecord {
                provider: "vitna-runtime".to_string(),
                model_sku: "vitna/code".to_string(),
                routing_reason: "lane".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "0".repeat(64),
            isolation_label: isolation.to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![EvidenceItemRecord {
                evidence_id: "ev-1".to_string(),
                grade: "sandbox_captured".to_string(),
                description: "test".to_string(),
                artifact_digest: None,
            }],
            changeset: ChangeSetRecord {
                files_modified: vec![
                    FileModificationRecord {
                        path: "src/a.rs".to_string(),
                        preimage_hash: "0".repeat(64),
                        postimage_hash: "1".repeat(64),
                    },
                    FileModificationRecord {
                        path: "src/shared.rs".to_string(),
                        preimage_hash: "0".repeat(64),
                        postimage_hash: "2".repeat(64),
                    },
                ],
                diff_digest: "c".repeat(64),
            },
            runner_execution_statements: vec![],
            egress_calls: vec![],
            child_receipt_roots: vec![],
            device_public_key: None,
            mandate_provenance: Some(MandateProvenanceRecord {
                chain_hash: "sha256:abc".to_string(),
                root_jti: "mnd-1".to_string(),
                path: vec!["mnd-1".to_string()],
                ledger: "https://ledger.local".to_string(),
                currency: "USD".to_string(),
                total_spent: "0.25".to_string(),
                spend_digests: vec![],
            }),
            device_signature: String::new(),
        }
    }

    #[test]
    fn diff_receipts_flags_regression_file_deltas_and_new_egress_domains() {
        let a = sample_receipt("run-a", "guarded");
        let mut b = sample_receipt("run-b", "full_access");
        b.model_selection.provider = "openai".to_string();
        b.model_selection.model_sku = "gpt-5".to_string();
        b.evidence_items.clear();
        b.changeset.files_modified = vec![
            FileModificationRecord {
                path: "src/shared.rs".to_string(),
                preimage_hash: "0".repeat(64),
                postimage_hash: "9".repeat(64),
            },
            FileModificationRecord {
                path: "src/new.rs".to_string(),
                preimage_hash: "0".repeat(64),
                postimage_hash: "3".repeat(64),
            },
        ];
        b.egress_calls.push(EgressCallRecord {
            target_domain: "api.example.com".to_string(),
            method: "GET".to_string(),
            redacted_url: "https://api.example.com/v1".to_string(),
            request_sha256: "1".repeat(64),
            status_code: 200,
            response_sha256: "2".repeat(64),
            bytes_sent: 10,
            bytes_received: 20,
            latency_ms: 15,
        });
        b.mandate_provenance.as_mut().unwrap().total_spent = "1.50".to_string();

        let d = diff_receipts(&a, &b);
        assert!(d.model.changed);
        assert!(d.isolation.changed);
        assert!(d.isolation.regression);
        assert_eq!(d.files.only_in_a, vec!["src/a.rs"]);
        assert_eq!(d.files.only_in_b, vec!["src/new.rs"]);
        assert_eq!(d.files.postimage_changed, vec!["src/shared.rs"]);
        assert_eq!(d.evidence.highest_grade_a, "sandbox_captured");
        assert_eq!(d.evidence.highest_grade_b, "none");
        assert_eq!(d.egress.new_domains, vec!["api.example.com"]);
        assert_eq!(d.mandate.spent_a.as_deref(), Some("0.25 USD"));
        assert_eq!(d.mandate.spent_b.as_deref(), Some("1.50 USD"));
    }
}
