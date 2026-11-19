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

    /// Runs all benchmarks and aggregates into a `BenchmarkReport` stamped with
    /// live UTC time and host/build metadata.
    pub fn run_all() -> BenchmarkReport {
        let metrics = vec![
            Self::bench_receipt_verification(500),
            Self::bench_merkle_root_calculation(100),
            Self::bench_dag_validation(500),
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
