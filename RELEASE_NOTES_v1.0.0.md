# Vitna Code v1.0.0: The Coding Terminal That Leaves a Receipt

- Status: Official Release (v1.0.0 General Availability)
- Date: 2026-09-16
- Repository: convexityos/vitna-code
- Working line: The coding terminal that leaves a receipt.

## 1. Executive Summary

Today we release Vitna Code v1.0.0, a local-first coding agent runtime engineered with an authoritative Rust core, a hardened 3-process execution boundary, and an append-only cryptographic evidence ledger.

Modern agentic coding products have created an engineering confidence crisis. When models modify repositories through unconstrained shell loops, developers are left reviewing sprawling git diffs without knowing what commands were run, what environment secrets were exposed, or whether the tests actually passed.

Vitna Code solves this by treating every software change as a verifiable claim backed by tamper-evident receipts:
- Every model selection records an explicit routing policy decision.
- Every tool action generates a deterministic digest and requires exact-action authority.
- Every modified file records cryptographic preimage and postimage SHA-256 hashes.
- Every test outcome is recorded as sandbox-captured evidence.
- Every completed run produces an Ed25519-signed receipt verifiable offline by any developer or CI runner.

---

## 2. Core Architectural Pillars

### 2.1 Local-First Daemon and Authoritative Rust Core
Vitna Code does not fork foreign runtimes or wrap web browsers in desktop shells. The entire core is built in memory-safe Rust across 18 specialized workspace crates and 2 native applications:
- Three-process isolation boundary:
  - `vitna-coded`: Local daemon managing sessions, policies, and event sequencing.
  - `vitna-runner`: Hardened execution process operating inside restricted OS sandboxes.
  - `vitna-git-broker`: Independent file and workspace coordinator computing preimages and postimages.
- Single-writer SQLite WAL storage actor: all operations append to a tamper-evident event log with zero disk lock contention.
- Interrupted actions transition cleanly to `needs_reconciliation` without blind retries.

### 2.2 The Cryptographic Evidence Ledger
At the heart of Vitna is `VitnaRunReceiptV1`:
- Deterministic RFC 8785 canonical JSON serialization.
- SHA-256 Merkle root covering the entire execution event hash chain.
- Ed25519 device key signing.
- Typed evidence grades distinguishing unverified assertions from attested facts:
  - `model_reported`: Unchecked model statements.
  - `broker_observed`: Observations recorded by the Git broker.
  - `sandbox_captured`: Direct execution captures and test exits.
  - `independently_reproduced`: Re-executed validations by secondary verifiers.
  - `remote_or_hardware_attested`: Signed device or cloud enclave attestations.
- Zero cloud dependency: receipts can be verified completely offline using `vitna verify <receipt.json>`.

### 2.3 Hardened OS Sandboxes and Credential Vaults
Vitna enforces security outside the model:
- Linux: Bubblewrap unsharing network, PID, and IPC namespaces with read-only system binds.
- macOS: Seatbelt profiles enforcing granular path confinement and network restrictions.
- Windows: AppContainer profile identity mapping with bounded execution timeouts and ambient secret scrubbing.
- Local OS keychain storage (`KeyringStore`) protecting API credentials (`wincred`, macOS Keychain, Secret Service) with zero plaintext disk leakage.

### 2.4 Durable Multi-Agent Coordination
Unlike systems where multiple agents corrupt a single git tree, Vitna Code enforces clean multi-agent isolation:
- Directed Acyclic Graph (DAG) task scheduler: validates acyclic dependencies via Kahn's algorithm, tracks ready tasks, and cascades cancellations if prerequisites fail.
- Per-agent disposable clone workspaces: child agents work inside isolated clones.
- Serialized merge queue (`MergeQueue`): re-validates preimages against the primary repository before applying changes. If a sibling agent has touched overlapping code, the conflict is flagged explicitly.
- Hierarchical receipt aggregation: child event Merkle roots and evidence records are collected into a signed parent coordinator receipt.

### 2.5 Extensibility with Model Context Protocol (MCP)
- Full Model Context Protocol (MCP) JSON-RPC 2.0 client.
- External tools bridged into Vitna's exact-action capability model.
- Tool arguments digested into `SHA256(mcp:{server}:{tool}:{args})` requiring explicit operator approval before execution.

---

## 3. Native Interfaces

### 3.1 Non-Interactive CLI (`vitna`)
- `vitna run <prompt>`: Headless execution for scripting, batch jobs, and CI/CD pipelines.
- `vitna doctor`: System diagnostics verifying sandbox readiness, git configuration, and provider credentials.
- `vitna verify <receipt.json>`: Standalone offline receipt verification.
- `vitna serve`: Background daemon listener over named pipes / domain sockets.
- `vitna sessions`: Session inspection and audit logs.

### 3.2 Calm Terminal TUI (`vitna-tui`)
- Dark-only terminal interface built with Ratatui and Crossterm.
- Adheres to the Calm Terminal design discipline: no frosted glass, no glow, no paper material.
- Single amber signal budget for operator attention.
- Structured human-in-the-loop modal displaying exact unified diffs, preimages, and postimages before any disk mutation.

---

## 4. Tier 1 Platform Support

Vitna Code v1.0.0 is tested and distributed across five Tier 1 platform architectures:
1. `x86_64-pc-windows-msvc` (Windows 11 / Windows 10 Build 19041+)
2. `aarch64-pc-windows-msvc` (Windows 11 on ARM64 Build 22000+)
3. `x86_64-unknown-linux-gnu` (Linux kernel 5.15+, glibc 2.31+)
4. `x86_64-apple-darwin` (macOS Monterey 12.0+)
5. `aarch64-apple-darwin` (macOS Monterey 12.0+ on Apple Silicon)

Each release archive includes cryptographic SHA-256 checksums in `SHA256SUMS` and an Ed25519-signed release manifest (`releases/v1.0.0/manifest.json`).

---

## 5. Performance and Integrity Benchmarks

Baseline evaluation results from `evals/results/benchmark_report_v1.0.0.json`:
- Offline receipt verification: **8,483 receipts / sec** (117.88 microseconds average latency).
- Merkle root computation over 1,000 events: **708 trees / sec** (1.41 milliseconds latency).
- DAG dependency sorting & cycle detection (50 nodes): **35,186 graphs / sec** (28.42 microseconds latency).
- Zero external copyleft dependencies: full SPDX 2.3 SBOM cataloging 32 packages under Apache-2.0 and MIT licenses.

---

## 6. Installation & Quickstart

### Download Binaries
Download the distribution archive for your architecture from the release assets or run the packaging script:
```bash
# Verify release manifest
vitna verify releases/v1.0.0/manifest.json
```

### Quickstart
```bash
# Run doctor check
vitna doctor

# Launch Calm Terminal interactive session
vitna-tui

# Or execute a verified prompt headlessly
vitna run "Refactor logging in src/main.rs to use structured tracing"
```
