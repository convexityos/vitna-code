use serde::{Deserialize, Serialize};
use std::time::Instant;
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
    pub metrics: Vec<BenchmarkMetric>,
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
            child_receipt_roots: Vec::new(),
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
        let ops_per_sec = if elapsed_secs > 0.0 { (iterations as f64) / elapsed_secs } else { 0.0 };
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
        let sample_hashes: Vec<String> = (0..1000)
            .map(|i| format!("{:064x}", i))
            .collect();

        let start = Instant::now();
        for _ in 0..iterations {
            let _root = VitnaRunReceiptV1::compute_event_merkle_root(&sample_hashes);
        }
        let elapsed = start.elapsed().as_nanos();
        let elapsed_secs = (elapsed as f64) / 1_000_000_000.0;
        let ops_per_sec = if elapsed_secs > 0.0 { (iterations as f64) / elapsed_secs } else { 0.0 };
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
        let ops_per_sec = if elapsed_secs > 0.0 { (iterations as f64) / elapsed_secs } else { 0.0 };
        let avg_latency_micros = (elapsed as f64) / (iterations as f64) / 1_000.0;

        BenchmarkMetric {
            name: "dag_kahn_validation_50_nodes".to_string(),
            operations: iterations,
            elapsed_nanos: elapsed,
            ops_per_sec,
            avg_latency_micros,
        }
    }

    /// Runs all benchmarks and aggregates into a BenchmarkReport.
    pub fn run_all() -> BenchmarkReport {
        let mut metrics = Vec::new();
        metrics.push(Self::bench_receipt_verification(500));
        metrics.push(Self::bench_merkle_root_calculation(100));
        metrics.push(Self::bench_dag_validation(500));

        BenchmarkReport {
            suite_version: "vitna-benchmark-v1.0.0".to_string(),
            timestamp_rfc3339: "2026-09-16T22:00:00Z".to_string(),
            metrics,
        }
    }
}
