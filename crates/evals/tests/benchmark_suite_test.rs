use vitna_evals::BenchmarkSuite;

#[test]
fn test_benchmark_suite_execution() {
    let report = BenchmarkSuite::run_all();

    assert_eq!(report.suite_version, "vitna-benchmark-v1.0.0");
    assert_eq!(report.metrics.len(), 3);

    for m in &report.metrics {
        assert!(m.operations > 0);
        assert!(m.ops_per_sec > 0.0);
        assert!(m.avg_latency_micros > 0.0);
    }

    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    assert!(json.contains("receipt_verification"));
    assert!(json.contains("merkle_root_calculation_1000_events"));
    assert!(json.contains("dag_kahn_validation_50_nodes"));
}
