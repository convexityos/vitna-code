//! Deterministic evals, fault-injection harnesses, and benchmark runners.

pub mod benchmark;

pub use benchmark::{BenchmarkMetric, BenchmarkReport, BenchmarkSuite};

pub struct EvalRunner;