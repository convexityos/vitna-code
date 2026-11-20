use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use vitna_orchestration::{TaskGraph, TaskNode};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::{
    generate_signing_key, ChangeSetRecord, ModelSelectionRecord, VitnaRunReceiptV1,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkMetric {
    pub name: String,
    pub operations: usize,
    pub elapsed_nanos: u128,
    pub ops_per_sec: f64,
    pub avg_latency_micros: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub suite_version: String,
    pub timestamp_rfc3339: String,
    pub os: String,
    pub arch: String,
    pub build_profile: String,
    pub metrics: Vec<BenchmarkMetric>,
}

/// Formats `SystemTime::now()` as a UTC RFC 3339 timestamp (`YYYY-MM-DDTHH:MM:SSZ`)
/// without requiring an external datetime crate.
pub fn utc_now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_secs_rfc3339(secs)
}

/// Converts Unix epoch seconds (UTC) into `YYYY-MM-DDTHH:MM:SSZ` using Howard
/// Hinnant's civil-from-days algorithm.
pub fn format_unix_secs_rfc3339(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = y + if m <= 2 { 1 } else { 0 };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

pub struct BenchmarkSuite;

impl BenchmarkSuite {
    /// Benchmarks offline cryptographic receipt verification throughput and latency.
    pub fn bench_receipt_verification(iterations: usize) -> BenchmarkMetric {
        let key = generate_signing_key();
        let pub_hex = hex::encode(key.verifying_key().to_bytes());

        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-bench-01".to_string(),
            session_id: "sess-bench-01".to_string(),
            workspace_fingerprint: "ws-bench-fp".to_string(),
            base_commit_sha: "abcd1234abcd1234abcd1234abcd1234abcd1234".to_string(),
            model_selection: ModelSelectionRecord {
                provider: "fake".to_string(),
                model_sku: "bench-model".to_string(),
                routing_reason: "benchmark".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "merkle-root-bench".to_string(),
            isolation_label: "strong".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: Vec::new(),
            changeset: ChangeSetRecord {
                files_modified: Vec::new(),
                diff_digest: "diff-digest-bench".to_string(),
            },
            runner_execution_statements: Vec::new(),
            egress_calls: Vec::new(),
            child_receipt_roots: Vec::new(),
            device_public_key: None,
            mandate_provenance: None,
            device_signature: String::new(),
        };

        receipt.sign(&key).expect("sign bench receipt");

        let start = Instant::now();
        for _ in 0..iterations {
            let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
            assert!(report.is_valid);
        }
        let elapsed = start.elapsed().as_nanos();
        let elapsed_secs = (elapsed as f64) / 1_000_000_000.0;
        let ops_per_sec = if elapsed_secs > 0.0 {
            (iterations as f64) / elapsed_secs
        } else {
            0.0
        };
        let avg_latency_micros = (elapsed as f64) / (iterations as f64) / 1_000.0;

        BenchmarkMetric {
            name: "receipt_verification".to_string(),
            operations: iterations,
            elapsed_nanos: elapsed,
            ops_per_sec,
            avg_latency_micros,
        }
    }

    /// Benchmarks event Merkle root calculation over 1,000 hashes.
    pub fn bench_merkle_root_calculation(iterations: usize) -> BenchmarkMetric {
        let sample_hashes: Vec<String> = (0..1000).map(|i| format!("{:064x}", i)).collect();

        let start = Instant::now();
        for _ in 0..iterations {
            let _root = VitnaRunReceiptV1::compute_event_merkle_root(&sample_hashes);
        }
        let elapsed = start.elapsed().as_nanos();
        let elapsed_secs = (elapsed as f64) / 1_000_000_000.0;
        let ops_per_sec = if elapsed_secs > 0.0 {
            (iterations as f64) / elapsed_secs
        } else {
            0.0
        };
        let avg_latency_micros = (elapsed as f64) / (iterations as f64) / 1_000.0;

        BenchmarkMetric {
            name: "merkle_root_calculation_1000_events".to_string(),
            operations: iterations,
            elapsed_nanos: elapsed,
            ops_per_sec,
            avg_latency_micros,
        }
    }

    /// Benchmarks DAG cycle detection and topological sorting.
    pub fn bench_dag_validation(iterations: usize) -> BenchmarkMetric {
        let mut graph = TaskGraph::new();
        for i in 0..50 {
            let mut node = TaskNode::new(format!("task-{}", i), "worker", "execute work");
            if i > 0 {
                node = node.with_dependency(format!("task-{}", i - 1));
            }
            if i > 2 {
                node = node.with_dependency(format!("task-{}", i - 2));
            }
            let _ = graph.add_node(node);
        }

        let start = Instant::now();
        for _ in 0..iterations {
            let _ = graph.validate();
        }
        let elapsed = start.elapsed().as_nanos();
        let elapsed_secs = (elapsed as f64) / 1_000_000_000.0;
        let ops_per_sec = if elapsed_secs > 0.0 {
            (iterations as f64) / elapsed_secs
        } else {
            0.0
        };
        let avg_latency_micros = (elapsed as f64) / (iterations as f64) / 1_000.0;

        BenchmarkMetric {
            name: "dag_kahn_validation_50_nodes".to_string(),
            operations: iterations,
            elapsed_nanos: elapsed,
            ops_per_sec,
            avg_latency_micros,
        }
    }

    /// Benchmarks 2-hop Ed25519 `am/1` Agent Mandate delegation chain verification.
    pub fn bench_mandate_chain_verification(iterations: usize) -> BenchmarkMetric {
        use std::collections::HashSet;
        use vitna_policy::generate_policy_signing_key;
        use vitna_policy::mandate::{
            did_key_from_verifying_key, jwk_thumbprint_from_verifying_key, sign_mandate_jws,
            verify_mandate_chain, CnfClaim, Effect, MandateActionRequest, MandateCaveats,
            MandateClaims, Money, VerifyMandateOptions, DEFAULT_SKEW_SECONDS, PROTOCOL_VERSION,
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let root_key = generate_policy_signing_key();
        let root_did = did_key_from_verifying_key(&root_key.verifying_key());
        let agent_key = generate_policy_signing_key();
        let agent_did = did_key_from_verifying_key(&agent_key.verifying_key());
        let agent_jkt = jwk_thumbprint_from_verifying_key(&agent_key.verifying_key());
        let leaf_key = generate_policy_signing_key();
        let leaf_did = did_key_from_verifying_key(&leaf_key.verifying_key());
        let leaf_jkt = jwk_thumbprint_from_verifying_key(&leaf_key.verifying_key());
        let ledger = "https://ledger.convexityos.com".to_string();

        let root_claims = MandateClaims {
            v: PROTOCOL_VERSION.to_string(),
            typ: "mandate".to_string(),
            jti: "mnd_bench_root".to_string(),
            iss: root_did.clone(),
            sub: agent_did.clone(),
            par: None,
            iat: now.saturating_sub(60),
            nbf: now.saturating_sub(60),
            exp: now + 3600,
            cnf: CnfClaim { jkt: agent_jkt },
            ledger: ledger.clone(),
            cav: MandateCaveats {
                max_total: Some(Money::new("USD", "5.00")),
                max_per_txn: Some(Money::new("USD", "1.00")),
                aud: Some(vec!["api.example.com".to_string()]),
                resources: Some(vec!["https://api.example.com/*".to_string()]),
                effect: Some(Effect::Write),
                currency: Some("USD".to_string()),
                delegable: Some(true),
                depth: Some(2),
                ..Default::default()
            },
            st: None,
        };
        let child_claims = MandateClaims {
            v: PROTOCOL_VERSION.to_string(),
            typ: "mandate".to_string(),
            jti: "mnd_bench_child".to_string(),
            iss: agent_did,
            sub: leaf_did,
            par: Some("mnd_bench_root".to_string()),
            iat: now.saturating_sub(30),
            nbf: now.saturating_sub(30),
            exp: now + 1800,
            cnf: CnfClaim { jkt: leaf_jkt },
            ledger: ledger.clone(),
            cav: MandateCaveats {
                max_total: Some(Money::new("USD", "1.00")),
                max_per_txn: Some(Money::new("USD", "0.25")),
                aud: Some(vec!["api.example.com".to_string()]),
                resources: Some(vec!["https://api.example.com/search".to_string()]),
                effect: Some(Effect::Read),
                currency: Some("USD".to_string()),
                delegable: Some(false),
                depth: Some(0),
                ..Default::default()
            },
            st: None,
        };
        let tokens = vec![
            sign_mandate_jws(&root_claims, &root_key, None),
            sign_mandate_jws(&child_claims, &agent_key, None),
        ];
        let mut trusted = HashSet::new();
        trusted.insert(root_did);
        let opts = VerifyMandateOptions {
            trusted_root_issuers: trusted,
            ledger,
            now,
            skew_seconds: DEFAULT_SKEW_SECONDS,
            request: Some(MandateActionRequest {
                resource: "https://api.example.com/search".to_string(),
                amount: Money::new("USD", "0.05"),
                effect: Some(Effect::Read),
                purpose: None,
                task_id: None,
            }),
            step_up_approved: false,
        };

        let start = Instant::now();
        for _ in 0..iterations {
            let verified = verify_mandate_chain(&tokens, &opts).expect("verify mandate chain");
            assert_eq!(verified.path.len(), 2);
        }
        let elapsed = start.elapsed().as_nanos();
        let elapsed_secs = (elapsed as f64) / 1_000_000_000.0;
        let ops_per_sec = if elapsed_secs > 0.0 {
            (iterations as f64) / elapsed_secs
        } else {
            0.0
        };
        let avg_latency_micros = (elapsed as f64) / (iterations as f64) / 1_000.0;

        BenchmarkMetric {
            name: "mandate_chain_verification_2_hops".to_string(),
            operations: iterations,
            elapsed_nanos: elapsed,
            ops_per_sec,
            avg_latency_micros,
        }
    }

    /// Runs all benchmarks with explicit iteration counts.
    pub fn run_with_iterations(
        receipt_iters: usize,
        merkle_iters: usize,
        dag_iters: usize,
        mandate_iters: usize,
    ) -> BenchmarkReport {
        let metrics = vec![
            Self::bench_receipt_verification(receipt_iters.max(1)),
            Self::bench_merkle_root_calculation(merkle_iters.max(1)),
            Self::bench_dag_validation(dag_iters.max(1)),
            Self::bench_mandate_chain_verification(mandate_iters.max(1)),
        ];

        BenchmarkReport {
            suite_version: "vitna-benchmark-v1.0.0".to_string(),
            timestamp_rfc3339: utc_now_rfc3339(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            build_profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
            metrics,
        }
    }

    /// Runs all benchmarks and aggregates into a `BenchmarkReport` stamped with
    /// live UTC time and host/build metadata.
    pub fn run_all() -> BenchmarkReport {
        Self::run_with_iterations(500, 100, 500, 200)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_unix_secs_matches_known_dates() {
        assert_eq!(format_unix_secs_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_unix_secs_rfc3339(1_700_000_000),
            "2023-11-14T22:13:20Z"
        );
        // Leap day 2024-02-29T12:34:56Z
        assert_eq!(
            format_unix_secs_rfc3339(1_709_210_096),
            "2024-02-29T12:34:56Z"
        );
    }
}
