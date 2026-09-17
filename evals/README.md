# Vitna Code Evaluation Suites

This directory contains evaluation benchmarks, golden fixtures, and fault injection matrices for Vitna Code.

## 1. Deterministic Engine Suite
Tests provider stream decoding, fake tools, schema migrations, WAL event replay, context compaction lineage, and protocol negotiation.

## 2. Reliability Fault Injection Suite
Simulates process death (`SIGKILL`) of daemon and runner at every transition state (prepared, started, finished, acknowledging). Injects network drops, disk exhaustion, clock skew, and invalid authentication tokens.

## 3. Security Corpus
Hostile repository tests:
- Path traversal escapes (`../`, symlink loops, NTFS junctions)
- Unauthorized Git hooks, filters, and aliases
- Terminal escape sequence injection (OSC clipboard hijack attempts)
- Command injection and shell parameter concatenation
- MCP permission escalation attempts
- Environment secret extraction attempts

## 4. Capability Benchmarks
Evaluates real repository tasks across TypeScript, Python, Rust, and Go against acceptance contracts and test suites.
