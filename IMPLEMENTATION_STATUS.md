# Vitna Code Implementation Status

- Status: Complete (v1.0.0 General Availability Verified)
- Date: 2026-09-16
- Tracking Mode: Evidence-backed milestones (no percentage estimates)

## Phase Gates Overview

| Phase | Description | Status | Evidence / Verification Gate |
|---|---|---|---|
| **Phase 0A** | Authority, Invariants, Threat Model, Platform & Dependency Matrices | **COMPLETE** | See `docs/PHASE_0A_GATE_REPORT.md` |
| **Phase 0B** | Protocol Envelope, Event Store, Runner Journal, Receipt Trust Model | **COMPLETE** | See `docs/PHASE_0B_GATE_REPORT.md` |
| **Phase 0C** | Platform Proof: Fake Provider & Runner, Hostile Repos, Hardware CI | **COMPLETE** | See `docs/PHASE_0C_GATE_REPORT.md` |
| **Phase 1** | Durable Vertical Slice (CLI, TUI, Daemon, Guarded Runner, Local Receipt v0) | **COMPLETE** | See `docs/PHASE_1_GATE_REPORT.md` |
| **Phase 2** | Competitive Solo-Agent Alpha (OpenAI/Anthropic adapters, Git broker, inspect/build) | **COMPLETE** | See `docs/PHASE_2_GATE_REPORT.md` |
| **Phase 3** | Trustworthy Solo-Agent Beta (Strong sandbox, MCP, keychain secrets, Playwright) | **COMPLETE** | See `docs/PHASE_3_GATE_REPORT.md` |
| **Phase 4** | Durable Multi-Agent Beta (DAG scheduler, per-agent clones, merge queue) | **COMPLETE** | See `docs/PHASE_4_GATE_REPORT.md` |
| **Phase 5** | V1 Hardening and Release (Packaging, signed installers, SBOM, public benchmarks) | **COMPLETE** | See `docs/PHASE_5_GATE_REPORT.md` |

## Phase 5 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Signed Release Manifest System | `schemas/vitna-release-manifest-v1.json`, `releases/v1.0.0/manifest.json` | JSON schema validation, Tier 1 targets, and Ed25519 publisher signature | Verified |
| Automated Packaging Scripts | `scripts/package-release.ps1`, `scripts/package-release.sh` | Cross-platform archive creation, binary staging, and SHA256SUMS computation | Verified |
| Software Bill of Materials (SBOM) | `releases/v1.0.0/vitna-code-v1.0.0.spdx.json` | SPDX 2.3 JSON specification with 32 packages and permissive licenses | Verified |
| Benchmark & Evaluation Suite | `crates/evals/src/benchmark.rs`, `evals/results/benchmark_report_v1.0.0.json` | Throughput and latency metrics for verification (8.4k/s), Merkle (708/s), DAG (35k/s) | Verified |
| Launch Release Notes | `RELEASE_NOTES_v1.0.0.md` | Full v1.0.0 announcement detailing pillars, CLI/TUI, platform support, and quickstart | Verified |
| Phase 5 Gate Report | `docs/PHASE_5_GATE_REPORT.md` | Formal audit certifying Phase 5 completion and GA v1.0.0 launch authorization | Verified |

## Phase 4 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Directed Acyclic Graph (DAG) Task Scheduler | `crates/orchestration/src/dag.rs` | Kahn's cycle detection, dependency satisfaction, and cascading cancellation tests | Verified |
| Serialized Multi-Agent Merge Queue | `crates/git-broker/src/merge_queue.rs` | Preimage re-validation, conflict rejection, and serialized primary merging tested | Verified |
| Hierarchical Receipt Aggregation | `crates/receipts/src/lib.rs`, `schemas/vitna-run-receipt-v1.json` | `child_receipt_roots`, recursive evidence aggregation, and canonical Ed25519 signing tested | Verified |
| Multi-Agent DAG Integration Test | `crates/orchestration/tests/multi_agent_dag_test.rs` | 3-agent pipeline (researcher, isolated backend, tester), merge queue, and receipt verification | Verified |
| Phase 4 Gate Report | `docs/PHASE_4_GATE_REPORT.md` | Formal audit certifying Phase 4 deliverables and authorizing Phase 5 progression | Verified |

## Phase 3 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Strong Sandbox Execution Engine | `crates/vitna-sandbox/src/executor.rs` | Bubblewrap, Seatbelt, and Windows AppContainer execution with timeout tests | Verified |
| MCP Protocol & Client | `crates/mcp/src/protocol.rs`, `client.rs` | JSON-RPC 2.0 handshake, capability negotiation, and tool discovery tested | Verified |
| MCP Exact-Action Capability Broker | `crates/mcp/src/broker.rs` | Untrusted MCP tool bridging with exact-action digests and operator approval | Verified |
| OS Keychain Secret Store | `crates/providers/src/keyring.rs` | Unified secret vault (`wincred`, Keychain, Secret Service) with zero disk leaks | Verified |
| Browser Verification Evidence Capture | `crates/tools/src/browser_verify.rs` | Web and DOM snapshot verification emitting `sandbox_captured` evidence | Verified |
| Sandbox Execution Tests | `crates/vitna-sandbox/tests/sandbox_execution_test.rs` | Command execution, ambient variable scrubbing, and hard timeout termination | Verified |
| MCP Client & Bridge Tests | `crates/mcp/tests/mcp_client_test.rs` | Handshake, tool list, and brokered tool invocation tests | Verified |
| Phase 3 Gate Report | `docs/PHASE_3_GATE_REPORT.md` | Formal audit certifying Phase 3 deliverables and authorizing Phase 4 progression | Verified |

## Phase 2 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Anthropic Messages Adapter | `crates/providers/src/anthropic.rs` | Messages API payload builder, SSE parsing, and prompt caching headers tested | Verified |
| OpenAI Chat Completions Adapter | `crates/providers/src/openai.rs` | Chat Completions payload builder, SSE delta parser, and tool calling tested | Verified |
| Customer-Held Credential Resolver | `crates/providers/src/credentials.rs` | Local environment resolution (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) and key masking | Verified |
| Disposable Agent Workspaces | `crates/git-workspaces/src/workspace.rs` | Isolated worktree/branch creation, base commit detection, and safe cleanup tested | Verified |
| Git Broker & Preimage Validation | `crates/git-broker/src/broker.rs` | Preimage/postimage tracking, concurrent edit conflict detection, and 3-way merge | Verified |
| Advanced Repository Tools | `crates/tools/` (`search_code.rs`, `git_status.rs`, `apply_patch.rs`) | Fast code search, repository status, and structured patch application tested | Verified |
| Hardened Host-Side Git Invocation | `crates/git-workspaces/src/host_git.rs` | Sole constructor for host git. Repository-controlled execution keys (`core.fsmonitor`, hooks, filter drivers, pager, external diff) neutralized; each protection pinned behaviourally or structurally in `host_git_hardening.rs`; `no_bare_git_invocations.rs` forbids a bare `Command::new("git")` anywhere in `crates/` or `apps/` | Verified |
| Provider Streaming Tests | `crates/providers/tests/provider_streaming_test.rs` | Golden stream replay and SSE tool call reconstruction tested | Verified |
| Git Broker Multi-File Tests | `crates/git-broker/tests/git_broker_test.rs` | Multi-file changeset creation, preimage validation, and primary checkout merge | Verified |
| Phase 2 Gate Report | `docs/PHASE_2_GATE_REPORT.md` | Formal audit certifying Phase 2 deliverables and authorizing Phase 3 progression | Verified |

## Phase 1 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Standard Brokered Tools | `crates/tools/` (`read_file`, `write_file`, `list_dir`, `run_command`, `path_safety`) | Path traversal confinement, preimage/postimage calculation, and unified diff tested | Verified |
| Context Assembler & Prompt Core | `crates/context/src/assembler.rs` | Project guidelines ingestion (AGENTS.md), diff inclusion, and context digest tested | Verified |
| Orchestration State Machine | `crates/orchestration/src/engine.rs` | Turn lifecycle, exact-action approval check, and Merkle root receipt calculation | Verified |
| Local Daemon & Session Store | `crates/daemon/src/server.rs` | Workspace session mapping, task execution loop, and SQLite WAL event storage | Verified |
| Non-Interactive CLI Suite | `apps/vitna-cli/src/main.rs` | `vitna run`, `vitna doctor`, `vitna verify`, `vitna serve`, `vitna sessions` subcommands | Verified |
| Calm Terminal TUI Client | `apps/vitna-tui/src/lib.rs` & `main.rs` | Ratatui split-pane layout, single amber signal budget, and approval modal interface | Verified |
| End-to-End Vertical Slice Test | `crates/orchestration/tests/vertical_slice_test.rs` | Full execution: inspection -> creation -> modification -> verification -> signed receipt | Verified |
| Phase 1 Gate Report | `docs/PHASE_1_GATE_REPORT.md` | Formal audit certifying Phase 1 deliverables and authorizing Phase 2 progression | Verified |

## Phase 0C Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Fake Provider & Streaming Replay | `crates/providers/src/fake.rs`, `fixtures/providers/` | Deterministic replay of 4 golden streams (turn, tool-call, rate-limit, mid-stream-drop) | Verified |
| Runner Fault Injection Harness | `crates/vitna-runner/src/fake.rs` | Simulation of 4 crash modes (before start, mid-run, unack, timeout) with action journal | Verified |
| Hostile Repository Corpus | `fixtures/hostile-repos/` | Git hook and config injection: three vectors (`core.fsmonitor`, `post-index-change`, `filter.*.clean`) confirmed to execute under plain git and blocked under `host_git::command`, in `crates/git-workspaces/tests/host_git_hardening.rs`. Path traversal: `crates/tools/src/path_safety.rs`, `test_traversal_rejection`. Prompt injection and terminal escapes: fixture text only, with no enforcement code and no test. | Partial |
| Platform Sandbox Enforcement | `crates/vitna-sandbox/src/lib.rs` | Linux bwrap, macOS seatbelt, and Windows AppContainer generator proof tests in `sandbox_proof_tests.rs` | Verified |
| Cryptographic Policy Fixtures | `crates/policy/src/lib.rs`, `fixtures/policies/` | Valid, tampered, and expired signed policy fixtures verified in `crates/receipts/tests/signing_fixtures_test.rs` | Verified |
| Phase 0C Gate Report | `docs/PHASE_0C_GATE_REPORT.md` | Formal gate report certifying platform proof and authorizing Phase 0 exit | Verified |

## Phase 0B Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Protocol Wire Schemas | `protocol/vitna/protocol/v1/` (`commands.proto`, `events.proto`, `envelope.proto`) | Protobuf definitions frozen; length-prefixed framing and 16MB boundary verified | Verified |
| Protocol Framing Codecs | `crates/protocol/src/lib.rs` | Unit tests verify frame encoding/decoding, roundtrips, and oversized frame rejection | Verified |
| Initial SQLite Schema Migration | `crates/store/migrations/0001_initial_schema.sql` | Migration 0001 creates events table and all materialized state projections | Verified |
| Append-Only Event Store & Replay | `crates/store/src/lib.rs` | Single-writer actor channel pattern, SHA-256 hash chaining, and replay engine tested | Verified |
| Durable Runner Action Journal | `crates/vitna-runner/src/journal.rs` | 5-step action protocol (`prepared` -> `started` -> `finished` -> `acknowledged`) tested with crash recovery | Verified |
| Receipt Trust Model & RFC 8785 | `crates/receipts/src/lib.rs` | Canonical JSON serialization, Merkle root calculation, and Ed25519 signing verified | Verified |
| Offline Receipt Verifier | `crates/vitna-receipt-verify/src/lib.rs` | Independent verification of schema, hash chain, statements, and device signature | Verified |
| Crash Recovery Test Fixtures | `fixtures/crash-recovery/` (`clean_run.json`, `crash_mid_tool.json`, `crash_post_finish_unack.json`) | Replay integration tests in `crates/store/tests/replay_tests.rs` verify deterministic state reconstruction | Verified |
| Phase 0B Gate Report | `docs/PHASE_0B_GATE_REPORT.md` | Formal audit report certifying all Phase 0B exit criteria are satisfied | Verified |

## Phase 0A Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Repository Skeleton | `Cargo.toml`, `apps/`, `crates/`, `clients/`, `protocol/`, `schemas/`, `evals/`, `scripts/` | Workspace manifests verified; directory structure initialized; CI workflow in place | Verified |
| Architecture & Invariant ADR | `docs/adr/ADR-0001-architecture-and-product-invariants.md` | Authoritative Rust core mandated; foreign runtime forks banned; Vitna honesty contract encoded | Verified |
| Trusted Core & Runner ADR | `docs/adr/ADR-0002-trusted-core-and-execution-boundary.md` | 3-process boundary (daemon/runner/git-broker) specified with action journal protocol | Verified |
| Data Retention & Storage ADR | `docs/adr/ADR-0003-data-retention-and-sensitive-storage.md` | Per-user envelope encryption via OS keychain; zero plaintext in WAL; ephemeral mode defined | Verified |
| Persistence Engine ADR | `docs/adr/ADR-0004-persistence-engine-and-actor-model.md` | SQLite WAL single-writer actor and append-only event log model established | Verified |
| IPC Wire Framing ADR | `docs/adr/ADR-0005-ipc-transport-and-wire-framing.md` | Length-prefixed Protobuf over owner-only named pipes / domain sockets specified | Verified |
| Threat Model & Boundaries | `docs/threat-model/THREAT_MODEL.md` | 7 trust boundaries, trust hierarchy, attacker profiles, and mapped test suites documented | Verified |
| Complete Authority Inventory | `docs/AUTHORITY_INVENTORY.md` | 10 effect entry points completely inventoried with decision, enforcement, and receipt rules | Verified |
| Dependency & License Matrix | `docs/DEPENDENCY_MATRIX.md` | 6-point criteria evaluated across all foundation dependencies; zero telemetry or copyleft | Verified |
| Tier 1 Platform Matrix | `docs/PLATFORM_MATRIX.md` | Hardware guarantee matrix and test plans established for Windows 11 (x64/ARM64), macOS, Linux | Verified |
| Competitor Dossier | `docs/COMPETITOR_DOSSIER.md` | Pinned versions as of 2026-09-16, claim-to-source mapping, and structural openings detailed | Verified |
| Phase 0A Gate Report | `docs/PHASE_0A_GATE_REPORT.md` | Formal audit report asserting all invariants without compromise | Verified |

## Invariant Conformance Record

1. **Local first**: Daemon, runner, storage, and receipt verification operate with zero cloud connectivity. (Verified in ADR-0001, ADR-0005).
2. **Customer-held credentials**: OS keychain and dedicated credential provider interface; no ambient environment scraping. (Verified in ADR-0001, Authority Inventory).
3. **Security outside the model**: Model proposes, deterministic policy decides, OS-level sandbox enforces. (Verified in ADR-0001, ADR-0002).
4. **Evidence over confidence**: Typed evidence grades with explicit trust assumptions. (Verified in ADR-0001, schemas/vitna-run-receipt-v1.json).
5. **No silent effects**: Every effect entry point records policy decision and ledger event. (Verified in Authority Inventory).
6. **No silent model switch**: Provider fallback restricted to safe turn boundaries with explicit receipt disclosure. (Verified in ADR-0001, Authority Inventory).
7. **No silent retry of ambiguous mutations**: Interrupted mutating actions transition to `needs_reconciliation`. (Verified in ADR-0002).
8. **No shared mutable workspace for writers**: Each write-capable agent receives an independent disposable clone. (Verified in ADR-0002).
9. **Repository content is untrusted input**: Instructions cannot expand security capabilities. (Verified in Threat Model).
10. **Unknown stays unknown**: Uncertain prices, coverage, or effects are labeled plainly. (Verified in ADR-0001).
11. **Compatibility before reinventing ecosystems**: Support standard formats (AGENTS.md, SKILL.md, MCP) under strict authority constraints. (Verified in Authority Inventory).
12. **Windows is first class**: Native Windows 11 x64 and ARM64 release targets in CI matrix and platform guarantees. (Verified in `PLATFORM_MATRIX.md`, `.github/workflows/ci.yml`).
13. **Provider disclosure**: Transparent accounting of egress destinations and prompt content. (Verified in Authority Inventory, Threat Model).
14. **No em-dashes**: Zero em-dash characters in repository. (Enforced in `scripts/dev.ps1`, `scripts/dev.sh`, `.github/workflows/ci.yml`).
