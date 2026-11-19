use vitna_evals::BenchmarkSuite;

#[test]
fn test_benchmark_suite_execution() {
    let report = BenchmarkSuite::run_all();

    assert_eq!(report.suite_version, "vitna-benchmark-v1.0.0");
    assert_eq!(report.metrics.len(), 3);

    // Must be a live UTC RFC 3339 timestamp, never the old hardcoded stub date.
    assert_ne!(report.timestamp_rfc3339, "2026-09-16T22:00:00Z");
    assert_eq!(report.timestamp_rfc3339.len(), 20);
    assert!(report.timestamp_rfc3339.ends_with('Z'));
    assert_eq!(&report.timestamp_rfc3339[4..5], "-");
    assert_eq!(&report.timestamp_rfc3339[7..8], "-");
    assert_eq!(&report.timestamp_rfc3339[10..11], "T");
    assert_eq!(&report.timestamp_rfc3339[13..14], ":");
    assert_eq!(&report.timestamp_rfc3339[16..17], ":");

    assert_eq!(report.os, std::env::consts::OS);
    assert_eq!(report.arch, std::env::consts::ARCH);
    assert!(report.build_profile == "debug" || report.build_profile == "release");

    for m in &report.metrics {
        assert!(m.operations > 0);
        assert!(m.ops_per_sec > 0.0);
        assert!(m.avg_latency_micros > 0.0);
    }

    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    assert!(json.contains("receipt_verification"));
    assert!(json.contains("merkle_root_calculation_1000_events"));
    assert!(json.contains("dag_kahn_validation_50_nodes"));
    assert!(json.contains("\"build_profile\""));
}
