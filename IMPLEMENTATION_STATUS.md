# Vitna Code Implementation Status

- Status: Pre-release. The v1.0.0 General Availability certification is withdrawn.
- Date: 2026-09-16, audited and corrected 2026-09-20
- Tracking Mode: Evidence-backed milestones (no percentage estimates)

## Audit note, 2026-09-20

This document certified "v1.0.0 General Availability Verified" on 2026-09-16 and
marked every deliverable across Phases 0A to 5 "Verified".

**The Rust workspace did not compile on that date, anywhere.** All five `Build &
Test` legs failed at manifest load in 6 to 24 seconds without reaching a line of
Rust, so none of the 54 tests the tree then held had ever run. It first compiled
on 2026-09-18 (#4). CI first concluded `success` on 2026-09-20 (#10, run
35546151349), because until then the Windows ARM64 leg named a runner label that
does not exist and was cancelled 24 hours later, which made every run in the
repository's history conclude `cancelled`.

A "Verified" written before the code could build attests that a file exists, not
that a claim is true. Every row below has been re-checked against what the tree
and its 142 passing tests now actually assert.

The phase gate reports are dated records of what was believed at the time. They
have not been rewritten, because how this happened is itself worth keeping. Each
one that overclaims now opens with a correction naming what it got wrong.

### The standard used here

| Status | Means |
|---|---|
| **Verified** | A test in this repository asserts the claim, and it runs in CI. |
| **Partial** | Some of the claim is asserted and some is not. The row says which. |
| **Smoke tested** | A happy-path test constructs the thing and exercises it once. None of the specific properties the row describes is asserted. |
| **Unverified** | No test asserts it. The code may well be correct; nothing here shows it. |

### What the audit found

1. **Phase 5's release evidence was never produced by running anything.** The
   benchmark report, the signed release manifest and its SHA-256 digests were
   committed on 2026-09-16 in a tree that could not build. See the Phase 5
   ledger below.
2. **The `v1.0.0` tag points at `cb35d7d`, a commit that does not compile.** No
   GitHub release exists, and the five archives the manifest hashes have never
   been built.
3. **Several surfaces marked Verified have no tests at all**: `apps/vitna-cli`
   (0), `crates/skills` (0) and `crates/telemetry` (0). `apps/vitna-tui`,
   `crates/daemon`, `crates/mcp`, `crates/context` and `crates/policy` have one
   lifecycle smoke test each.
4. **The invariant record cited design documents as verification.** An ADR
   records intent; it cannot verify an implementation. That section now names a
   test or says that nothing asserts the invariant.
5. **One tool fabricated its own evidence. Fixed on 2026-09-20 (#14).**
   `browser_verify` performed no network I/O. For an `http` or `https` target
   it built a fixed HTML string, hashed that as the "DOM Digest" it returned
   as a `postimage_hash`, and tested the caller's `expected_text` against the
   string it had just made up, so `expected_text: "App Loaded"` passed for any
   URL including an unreachable one. It was untested, it was not gated behind
   approval, and its output is shaped to land in a receipt. That was a live
   defect rather than a documentation error, and it was the same failure that
   #7 found in `run_command`: an evidence type asserted over something that
   did not happen. A remote target is now refused with an error naming the
   authority that is missing, and what remains is a digest of bytes this
   process read. The capability the tool is named for still does not exist
   anywhere in this build, which the Phase 3 ledger row below now states
   rather than implying it was delivered.

## Phase Gates Overview

| Phase | Description | Status | Evidence / Verification Gate |
|---|---|---|---|
| **Phase 0A** | Authority, Invariants, Threat Model, Platform & Dependency Matrices | **COMPLETE** | A documentation phase: every artifact exists. See `docs/PHASE_0A_GATE_REPORT.md` |
| **Phase 0B** | Protocol Envelope, Event Store, Runner Journal, Receipt Trust Model | **COMPLETE** | Framing, hash chain, journal and replay are all asserted by tests. See `docs/PHASE_0B_GATE_REPORT.md` |
| **Phase 0C** | Platform Proof: Fake Provider & Runner, Hostile Repos, Hardware CI | **PARTIAL** | Two of five deliverables are Partial. Hardware CI was not real until 2026-09-20: the Windows ARM64 leg had never executed. See `docs/PHASE_0C_GATE_REPORT.md` |
| **Phase 1** | Durable Vertical Slice (CLI, TUI, Daemon, Guarded Runner, Local Receipt v0) | **PARTIAL** | The slice runs end to end, but the CLI has no tests and the TUI and daemon have one smoke test each. See `docs/PHASE_1_GATE_REPORT.md` |
| **Phase 2** | Competitive Solo-Agent Alpha (OpenAI/Anthropic adapters, Git broker, inspect/build) | **COMPLETE** | The best covered phase: golden stream replay, and 86 tests across tools, git-broker and git-workspaces. See `docs/PHASE_2_GATE_REPORT.md` |
| **Phase 3** | Trustworthy Solo-Agent Beta (Strong sandbox, MCP, keychain secrets, Playwright) | **PARTIAL** | Strong isolation is not implemented and is refused rather than downgraded. Windows has no sandbox backend. MCP has one test. See `docs/PHASE_3_GATE_REPORT.md` |
| **Phase 4** | Durable Multi-Agent Beta (DAG scheduler, per-agent clones, merge queue) | **COMPLETE** | Cycle detection, cascading cancellation, merge queue and the 3-agent pipeline are asserted. See `docs/PHASE_4_GATE_REPORT.md` |
| **Phase 5** | V1 Hardening and Release (Packaging, signed installers, SBOM, public benchmarks) | **WITHDRAWN** | Its release evidence was committed without being produced. Nothing was built, measured or published. See `docs/PHASE_5_GATE_REPORT.md` |

## Phase 5 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Signed Release Manifest System | `schemas/vitna-release-manifest-v1.json`, `releases/v1.0.0/manifest.json` | The schema exists and the manifest parses against it. **Its contents describe files that have never existed.** It lists five archives with SHA-256 digests and byte sizes, committed 2026-09-16 in a tree that did not compile, so no binary was built to hash. No code reads this manifest and nothing verifies the `publisher_signature`, contrary to the Phase 5 report's claim that it was "verified offline". There is no GitHub release, and the `v1.0.0` tag points at `cb35d7d`, which does not build. | Unverified, and the digests are not real |
| Automated Packaging Scripts | `scripts/package-release.ps1`, `scripts/package-release.sh` | Both files exist. Neither is invoked by CI or by any test, and neither has been run in this repository: they are the scripts that would have produced the archives the manifest hashes, and those archives do not exist. | Unverified |
| Software Bill of Materials (SBOM) | `releases/v1.0.0/vitna-code-v1.0.0.spdx.json` | Valid SPDX 2.3 listing 32 packages, of which 20 are this workspace's own crates and applications. **The resolved dependency graph is 283 packages**, so it catalogs about 12 of roughly 263 external ones. The claim of "zero copyleft" cannot be supported from that sample, and `Cargo.lock` is in `.gitignore`, so the set is not pinned. Regenerating this from a committed lockfile is the fix. | Unverified, and materially incomplete |
| Benchmark & Evaluation Suite | `crates/evals/src/benchmark.rs`, `evals/results/benchmark_report_v1.0.0.json` | `test_benchmark_suite_execution` runs the suite and asserts only that there are three metrics and that each is greater than zero. It never compares against the committed report, so any positive numbers pass. **The committed figures (8,483/s, 708/s, 35,186/s) were never measured**: the report is stamped `2026-09-16T22:00:00Z` and was committed in `cb35d7d`, two days before the workspace first compiled, and it has not been regenerated since the code could run. It records no hardware, no toolchain and no build profile. | Unverified, and the figures are not measurements |
| Launch Release Notes | `RELEASE_NOTES_v1.0.0.md` | The document exists. It announces a GA release that did not happen, for binaries that were never built. | Unverified |
| Phase 5 Gate Report | `docs/PHASE_5_GATE_REPORT.md` | Exists, and is the document that certified the above. It now opens with a correction. | Corrected 2026-09-20 |

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
| Guarded Sandbox Planning | `crates/vitna-sandbox/src/lib.rs` | Decides the invocation that delivers the requested guarantee and refuses when it cannot. Bubblewrap (Linux) and Seatbelt (macOS) implemented; **Windows has no backend**, so `run_command` there is refused unless unsandboxed execution is explicitly approved. `strong` (container or VM) is not implemented and returns an error rather than being served by something weaker. | Partial |
| MCP Protocol & Client | `crates/mcp/src/protocol.rs`, `client.rs` | One test covers this crate: `test_mcp_client_handshake_and_tool_brokering`. It exercises a handshake and a brokered call on the happy path. Capability negotiation and tool discovery are implemented but nothing asserts them, and there is no test for a hostile or malformed server response. | Smoke tested |
| MCP Exact-Action Capability Broker | `crates/mcp/src/broker.rs` | Covered only by the same single test above, on its approval path. Nothing asserts that an unapproved action is refused, which is the property the broker exists for. | Smoke tested |
| OS Keychain Secret Store | `crates/providers/src/keyring.rs` | `keyring::tests::test_keyring_store_lifecycle` covers store and retrieve. "Zero disk leaks" is not asserted anywhere: no test looks for a secret written to disk, and none of the three named OS backends is exercised against a real keychain in CI. | Partial |
| Browser Verification Evidence Capture | `crates/tools/src/browser_verify.rs` | **The deliverable as named was never built, and #14 did not build it: no browser is involved and no page is fetched.** Until 2026-09-20 the tool fabricated its evidence for any URL. Given an `http` or `https` target it did not fetch it: it built the literal string `<html><body><h1>App Loaded</h1><p>Target: {url}</p></body></html>`, hashed that as the "DOM Digest" and returned it as the result's `postimage_hash`, then tested `expected_text` against the string it had just made up, so `expected_text: "App Loaded"` passed for every URL including an unreachable one. #14 refuses a remote target instead, with an error naming the authority that is missing, and closes a second hole found on the way: the local path was joined onto the workspace root and normalized nothing, so `../outside/secret.txt` and `/etc/passwd` both read outside the workspace on a tool that requires no approval. The path now resolves through the `workspace_fs` containment, a document larger than the 16 MiB the tool reads is refused rather than digested as a prefix, and `exit_code` is `None` because no process ran. The report states that the digest is of the bytes on disk and that no renderer ran, so a receipt cannot read it as a captured DOM. The file went from zero tests to fifteen (fourteen on Windows; the FIFO case is `cfg(unix)`), among them the regression itself: a URL nothing serves cannot produce a passing assertion. What is verified is a workspace document digest. Verifying a live page remains impossible here, since `net:http_fetch` has no broker and the `egress_call` event is unimplemented (`docs/AUTHORITY_INVENTORY.md` section 5), and the name `browser_verify` still overstates what the tool does. | Not delivered; the document digest that remains is Verified |
| Sandbox Enforcement Tests | `crates/vitna-runner/tests/sandbox_enforcement.rs` | Runs real commands through `ProcessRunner`: refusal when no backend exists, writes outside the workspace blocked, `.git/config` unwritable, process tree terminated on timeout (mutation checked: with the job object disabled the timed-out command survives and the test fails). `VITNA_REQUIRE_SANDBOX_BACKEND` turns a missing backend into a failure in CI instead of a skip. | Verified |
| MCP Client & Bridge Tests | `crates/mcp/tests/mcp_client_test.rs` | Handshake, tool list, and brokered tool invocation tests | Verified |
| Phase 3 Gate Report | `docs/PHASE_3_GATE_REPORT.md` | Formal audit certifying Phase 3 deliverables and authorizing Phase 4 progression | Verified |

## Phase 2 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Anthropic Messages Adapter | `crates/providers/src/anthropic.rs` | Messages API payload builder, SSE parsing, and prompt caching headers tested | Verified |
| OpenAI Chat Completions Adapter | `crates/providers/src/openai.rs` | Chat Completions payload builder, SSE delta parser, and tool calling tested | Verified |
| Customer-Held Credential Resolver | `crates/providers/src/credentials.rs` | Local environment resolution (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) and key masking | Verified |
| Disposable Agent Workspaces | `crates/git-workspaces/src/workspace.rs`, `workspace/materialize.rs` | Isolated worktree/branch creation, base commit detection, and safe cleanup tested. Materialization never follows a link: one is kept only when its text is relative and it resolves inside the checkout, and everything else (links out, absolute or dangling links, FIFOs, sockets, devices) is refused and recorded in `refused_entries`. A committed `.vitna` link cannot place the workspace, and `GitBroker::inspect_changes` skips links. Tests for links out, a cycle, device links and a FIFO run in CI on Linux, macOS and Windows x86_64 (device and FIFO cases on Unix only). | Verified |
| Git Broker & Preimage Validation | `crates/git-broker/src/broker.rs`, `tree.rs` | Preimage/postimage tracking and concurrent edit conflict detection; the merge copies files and does no 3-way merge. Neither the primary checkout nor the agent workspace is read or written through a link: a link on a changeset path, leading out of the checkout or staying inside it, or a directory, FIFO, socket or device at the path, fails `inspect_changes` and is a merge conflict found before anything is written, and each opened handle's own path is re-checked. Missing directories are created one at a time, never through a link. Only bytes that still hash to the changeset's postimage are merged, every source checked before the first write, and `apply_changeset` and the merge queue share one write path. Tests: `tests/primary_links_test.rs`, `tests/merge_source_test.rs` and `src/tree.rs` (file-symlink cases skip where links cannot be made; FIFO case on Unix only). The merge is not atomic: a write refused after validation, or an I/O error, can leave earlier files written, and the error names them. | Verified |
| Advanced Repository Tools | `crates/tools/` (`search_code.rs`, `git_status.rs`, `apply_patch.rs`) | Fast code search, repository status, and structured patch application tested | Verified |
| Hardened Host-Side Git Invocation | `crates/git-workspaces/src/host_git.rs` | Sole constructor for host git. Repository-controlled execution keys (`core.fsmonitor`, hooks, filter drivers, pager, external diff) neutralized; each protection pinned behaviourally or structurally in `host_git_hardening.rs`; `no_bare_git_invocations.rs` forbids a bare `Command::new("git")` anywhere in `crates/` or `apps/` | Verified |
| Provider Streaming Tests | `crates/providers/tests/provider_streaming_test.rs` | Golden stream replay and SSE tool call reconstruction tested | Verified |
| Git Broker Multi-File Tests | `crates/git-broker/tests/git_broker_test.rs` | Multi-file changeset creation, preimage validation, and primary checkout merge | Verified |
| Phase 2 Gate Report | `docs/PHASE_2_GATE_REPORT.md` | Formal audit certifying Phase 2 deliverables and authorizing Phase 3 progression | Verified |

## Phase 1 Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Standard Brokered Tools | `crates/tools/` (`read_file`, `write_file`, `apply_patch`, `list_dir`, `search_code`, `run_command`, `path_safety`, `workspace_fs`) | Lexical traversal rejection in `path_safety`, then containment on the real path for every file tool: links resolved, regular files only (no blocking on a FIFO, no following a final link), and the opened handle's own path re-checked (`/proc/self/fd`, `F_GETPATH`, `GetFinalPathNameByHandleW`). The postimage is hashed from the bytes read back after `sync_all`, and a mismatch fails the write (mutation checked); line endings are written byte-exact and kept in the diff. `read_file` loads at most 16 MiB and returns at most 256 KiB, with truncation notices; `search_code` reports unreadable paths as a partial result, never as no matches. Run in CI on Linux, macOS and Windows x86_64. Writes are in place, not atomic, and `run_command`'s working directory is still checked lexically only, its confinement being the runner sandbox's. | Verified |
| Context Assembler & Prompt Core | `crates/context/src/assembler.rs` | One test covers this crate: `test_context_assembler_lifecycle`. It exercises assembly once. Prompt injection through an ingested `AGENTS.md` is a Phase 0C fixture with no enforcement code and no test, so ingestion is covered but its hostile case is not. | Smoke tested |
| Orchestration State Machine | `crates/orchestration/src/engine.rs` | Turn lifecycle, exact-action approval check, and Merkle root receipt calculation | Verified |
| Local Daemon & Session Store | `crates/daemon/src/server.rs` | One test covers this crate: `test_daemon_server_session_and_run`, a happy-path session and run. The event store beneath it is separately and properly tested (`crates/store`, hash chain and replay). The daemon's own behaviour under a dropped connection, a concurrent session or a restart is not asserted. | Smoke tested |
| Non-Interactive CLI Suite | `apps/vitna-cli/src/main.rs` | **`apps/vitna-cli` contains no tests at all.** The subcommands exist and the crate compiles and clippy-passes on five platforms. No test invokes any subcommand, checks an exit code, or covers argument parsing. | Unverified |
| Calm Terminal TUI Client | `apps/vitna-tui/src/lib.rs` & `main.rs` | One test covers this crate: `test_terminal_app_lifecycle`. None of the three properties this row names is asserted by it: not the split-pane layout, not the single amber signal budget, and not the approval modal. | Smoke tested |
| End-to-End Vertical Slice Test | `crates/orchestration/tests/vertical_slice_test.rs` | Full execution: inspection -> creation -> modification -> verification -> signed receipt | Verified |
| Phase 1 Gate Report | `docs/PHASE_1_GATE_REPORT.md` | Formal audit certifying Phase 1 deliverables and authorizing Phase 2 progression | Verified |

## Phase 0C Deliverables Ledger

| Deliverable | Target Location | Verification Method | Status |
|---|---|---|---|
| Fake Provider & Streaming Replay | `crates/providers/src/fake.rs`, `fixtures/providers/` | Deterministic replay of 4 golden streams (turn, tool-call, rate-limit, mid-stream-drop) | Verified |
| Runner Fault Injection Harness | `crates/vitna-runner/src/fake.rs` | Simulation of 4 crash modes (before start, mid-run, unack, timeout) with action journal | Verified |
| Hostile Repository Corpus | `fixtures/hostile-repos/` | Git hook and config injection: three vectors (`core.fsmonitor`, `post-index-change`, `filter.*.clean`) confirmed to execute under plain git and blocked under `host_git::command`, in `crates/git-workspaces/tests/host_git_hardening.rs`. Path traversal: `crates/tools/src/path_safety.rs`, `test_traversal_rejection`. Prompt injection and terminal escapes: fixture text only, with no enforcement code and no test. | Partial |
| Platform Sandbox Argument Generation | `crates/vitna-sandbox/src/lib.rs` | `sandbox_proof_tests.rs` asserts on generated arguments and profiles only, and starts no process. Whether anything is actually confined is asserted separately by `crates/vitna-runner/tests/sandbox_enforcement.rs`. The Windows AppContainer entry generates a deterministic **name**, which is an identifier and not a container. | Partial |
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

Every line here except 3 and 12 used to end in "(Verified in ADR-0001)" or
similar. **An ADR records a decision. It cannot verify an implementation.**
Citing one as evidence that code upholds an invariant is a category error, and
it is the error that let this document certify a workspace that did not compile.

Each line now names a test that asserts the invariant, or says plainly that
nothing does. "No test asserts this" is not an accusation that the code is
wrong. It means the claim is currently a design intention.


1. **Local first**: Daemon, runner, storage, and receipt verification operate with zero cloud connectivity. **No test asserts this.** Nothing in the suite blocks or observes network egress, so "zero cloud connectivity" is a design intention that no run has demonstrated. Specified in ADR-0001 and ADR-0005.
2. **Customer-held credentials**: OS keychain and dedicated credential provider interface; no ambient environment scraping. **Partly asserted.** `credentials::tests::test_mask_key` and `keyring::tests::test_keyring_store_lifecycle` cover masking and a store/retrieve round trip. No test asserts the absence of ambient environment scraping, and no OS keychain backend is exercised against a real keychain in CI.
3. **Security outside the model**: Model proposes, deterministic policy decides, OS-level sandbox enforces. Enforced on Linux (bubblewrap) and macOS (Seatbelt). On Windows no sandbox backend exists, so the policy refuses the command rather than running it unconfined; that refusal is the enforcement there. (ADR-0001, ADR-0002, `crates/vitna-runner/tests/sandbox_enforcement.rs`.)
4. **Evidence over confidence**: Typed evidence grades with explicit trust assumptions. **Partly asserted.** `tests::test_receipt_canonicalization_and_signing` and `tests::test_verifier_report` cover canonical form, signing and offline verification. Nothing asserts that an evidence grade matches what actually happened. The live counterexample this line carried until 2026-09-20 is gone: `browser_verify` emitted a DOM digest of a document it had fabricated, and since #14 it refuses a remote target and digests only bytes it read, saying in its own report that no renderer ran. That closes one instance rather than the gap, which is still held by convention and not by a test. See the Phase 3 ledger.
5. **No silent effects**: Every effect entry point records policy decision and ledger event. **Asserted** by `journal::tests::test_action_journal_lifecycle`, `a_refused_action_is_journalled_as_refused` and `the_journal_records_the_isolation_that_applied`. The coverage is of the runner's journal; no test enumerates the ten effect entry points in `AUTHORITY_INVENTORY.md` and checks each one writes a record.
6. **No silent model switch**: Provider fallback restricted to safe turn boundaries with explicit receipt disclosure. **No test asserts this.** No test performs a provider fallback, so neither the turn boundary restriction nor the receipt disclosure has been exercised.
7. **No silent retry of ambiguous mutations**: Interrupted mutating actions transition to `needs_reconciliation`. **Asserted** by `journal::tests::test_crash_recovery_forces_reconciliation`, `tests::test_corrupt_hash_chain_forces_reconciliation` and `test_fixture_crash_post_finish_unack`, against the recorded crash-recovery fixtures.
8. **No shared mutable workspace for writers**: Each write-capable agent receives an independent disposable clone. **Asserted** by `workspace::tests::test_agent_workspace_lifecycle` and `workspace::tests::test_existing_workspace_directory_is_not_reused`, and by the multi-agent pipeline in `test_multi_agent_dag_execution_and_receipt_aggregation`.
9. **Repository content is untrusted input**: Instructions cannot expand security capabilities. **The best asserted invariant in the repository**, and only since 2026-09-20. `hardened_git_does_not_run_repository_controlled_commands` and `git_status_tool_does_not_run_repository_controlled_commands` confirm the three git execution keys fire under plain git and are blocked here; `no_crate_builds_a_bare_git_command` sweeps the source; and thirty-six tests across `workspace_fs`, `materialize`, `tree` and `path_safety` cover links, FIFOs, devices and post-check swaps. Before that date a repository could execute code through `git status` with operator privileges and no prompt.
10. **Unknown stays unknown**: Uncertain prices, coverage, or effects are labeled plainly. **No test asserts this.** This document is the clearest measure of how the invariant was actually held, and until this audit it was not: unverified claims were labelled Verified throughout.
11. **Compatibility before reinventing ecosystems**: Support standard formats (AGENTS.md, SKILL.md, MCP) under strict authority constraints. **Partly asserted.** `AGENTS.md` ingestion is covered by `tests::test_context_assembler_lifecycle` and MCP by a single handshake test. `crates/skills` contains no tests, so SKILL.md support is unverified.
12. **Windows is first class**: Native Windows 11 x64 and ARM64 release targets in CI matrix and platform guarantees. Windows ARM64 carried this claim unverified until 2026-09-20. From 2026-09-16 the ARM64 leg named the runner label `windows-11-arm64`, which GitHub does not publish, so no runner ever matched it: the job was queued and cancelled 24 hours later without executing a step, and every run in that window concluded `cancelled` while its other six jobs passed. The label is `windows-11-arm`. Run 35546151349 is the first execution of this leg, and it reports `cargo check`, `cargo clippy -- -D warnings` and 54 passed / 0 failed on `aarch64-pc-windows-msvc`, matching `x86_64-unknown-linux-gnu` test for test. (Verified in `PLATFORM_MATRIX.md`, `.github/workflows/ci.yml`, `scripts/check-runner-labels.mjs`).
13. **Provider disclosure**: Transparent accounting of egress destinations and prompt content. **No test asserts this.** `crates/telemetry` contains no tests, and nothing records or checks an egress destination during a run.
14. **No em-dashes**: Zero em-dash characters in repository. **Asserted**, and the only invariant that was genuinely enforced on every commit throughout: the `check-rules` job greps the whole tree and fails the build. Also in `scripts/dev.ps1` and `scripts/dev.sh`.
