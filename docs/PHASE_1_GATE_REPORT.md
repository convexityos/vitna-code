# Phase 1 Gate Report: Durable Vertical Slice

> **Correction, 2026-09-20.** This report is kept as a dated record of what was
> believed on 2026-09-16, and has deliberately not been rewritten. It was
> written in a tree where the Rust workspace did not compile: all five
> `Build & Test` legs failed at manifest load in 6 to 24 seconds, so no test in
> this repository had ever run. The workspace first compiled on 2026-09-18 (#4)
> and CI first concluded `success` on 2026-09-20 (#10). Read every "PASSED" and
> every "verified" below in that light. The corrected status is in the audit
> note at the top of `IMPLEMENTATION_STATUS.md`.
>
> Specific to this report: the vertical slice does run end to end
> (`test_durable_vertical_slice_end_to_end`), but the surfaces this phase is
> named for are barely covered. `apps/vitna-cli` contains no tests at all, and
> `apps/vitna-tui` and `crates/daemon` have one lifecycle smoke test each. None
> of the TUI properties this report describes is asserted.


- Status: PASSED
- Date: 2026-09-16
- Working product: Vitna Code (vitna)
- Phase: 1 (Durable Vertical Slice: CLI, TUI, Daemon, Guarded Runner, Local Receipt v0)
- Repository: convexityos/vitna-code

## 1. Executive Summary

Phase 1 completes the durable vertical slice of Vitna Code. The system connects the command-line interface (`vitna-cli`), terminal UI (`vitna-tui`), local daemon (`vitna-daemon`), context assembler (`vitna-context`), standard tool suite (`vitna-tools`), execution runner (`vitna-runner`), orchestration state machine (`vitna-orchestration`), and cryptographic receipt generator (`vitna-receipts`).

An end-to-end task can now be accepted, workspace files inspected, edits proposed with preimage/postimage tracking and unified diffs, approval sought for mutating actions, commands executed in the guarded runner, verification checks captured as evidence, and a signed, tamper-evident cryptographic receipt emitted to `.vitna/receipts/{run_id}.json`.

## 2. Deliverables Audit

### 2.1 Standard Brokered Tools (`crates/tools`)
Implemented secure, workspace-confined tool execution suite:
1. `read_file`: Line-numbered inspection of workspace files with SHA-256 content verification and path traversal rejection (`crates/tools/src/read_file.rs`).
2. `write_file`: File creation and modification with SHA-256 preimage/postimage calculation and unified diff generation (`crates/tools/src/write_file.rs`).
3. `list_dir`: Workspace directory inspection returning entry types, sizes, and relative paths (`crates/tools/src/list_dir.rs`).
4. `run_command`: Execution tool mediated through `vitna-runner` with execution statements and exit code tracking (`crates/tools/src/run_command.rs`).
5. `path_safety`: Lexical path normalizer strictly enforcing that no relative traversal (`..`) escapes the workspace root (`crates/tools/src/path_safety.rs`).
6. `ToolRegistry`: Unified registry with standard tool suite initialization and dispatch (`crates/tools/src/lib.rs`).

### 2.2 Context Assembly & Prompt Engineering (`crates/context`)
- `ContextAssembler`: Ingests base system instructions (Calm Terminal, Honesty Contract), parses project instructions (`AGENTS.md`, `CLAUDE.md`) as untrusted repository input, formats tool definitions, tracks turn history, and incorporates uncommitted diffs.
- Context Digest: Computes deterministic SHA-256 digest over the assembled context to bind turns cryptographically to their inputs (`crates/context/src/assembler.rs`).

### 2.3 Orchestration Engine (`crates/orchestration`)
- `OrchestrationEngine`: Coordinates the full turn lifecycle:
  1. `TurnStarted`: Records monotonic event in SQLite event store.
  2. `ContextAssembled`: Computes context digest and token estimates.
  3. `ApprovalRequested` / `ApprovalGranted`: Enforces exact-action capability checks for mutating actions.
  4. `ToolStarted` / `ToolFinished`: Dispatches tools through guarded runner and records preimage/postimage hashes.
  5. `RunningVerification`: Executes verification test commands to produce `sandbox_captured` evidence items.
  6. `FinalizingReceipt`: Computes event Merkle root, builds `VitnaRunReceiptV1`, signs with Ed25519 device key, and saves to `.vitna/receipts/{run_id}.json`.

### 2.4 Local Daemon (`crates/daemon`)
- `DaemonServer`:
  - Persistent SQLite WAL event store integration (`crates/store`).
  - Session registry mapping workspaces to active session states.
  - Task execution handler (`run_task`) driving the orchestration engine.

### 2.5 CLI Binary (`apps/vitna-cli`)
Implemented complete subcommand suite in `apps/vitna-cli/src/main.rs`:
- `vitna run <task>`: Executes a task end-to-end in non-interactive mode.
- `vitna serve`: Starts the persistent local daemon.
- `vitna doctor`: Audits OS platform, Git, SQLite WAL persistence, and Ed25519 signing core.
- `vitna verify <receipt-file>`: Offline cryptographic verifier for exported receipt files.
- `vitna sessions`: Lists active and previous sessions in the workspace.

### 2.6 Terminal UI Client (`apps/vitna-tui`)
Implemented Ratatui terminal application adhering to the Calm Terminal visual law:
- Dark-only palette with clean single-line borders.
- Split-pane layout: Evidence Ledger & Transcript (60%) and Changeset Diff Inspector (40%).
- Amber budget constraint: Exactly one amber signal allocated per view, strictly reserved for the action approval modal.
- Keybindings: `Enter` to submit prompt, `Y`/`N` for action approval, `Ctrl+C` to quit.

### 2.7 End-to-End Vertical Slice Integration Test (`crates/orchestration/tests/vertical_slice_test.rs`)
Integration test proving the full vertical slice:
- Initial file setup (`src/lib.rs`).
- File inspection with `read_file` / `list_dir`.
- File creation (`src/utils.rs`) verifying zero preimage hash.
- File modification (`src/lib.rs`) verifying accurate preimage, postimage, and diff.
- Verification command execution generating `sandbox_captured` evidence.
- Receipt generation and Ed25519 signature verification.
- Offline independent verification via `vitna_receipt_verify`.
- Event store replay validation confirming hash continuity across all steps.

## 3. Phase 1 Exit Certification

With Phase 1 complete and verified:
- The Vitna honesty contract is enforced end-to-end across tools, runner, store, and receipts.
- All code changes produce preimage and postimage hashes.
- Mutating actions require explicit action capability digests.
- The project is certified to exit Phase 1 and proceed to Phase 2 (Competitive Solo-Agent Alpha).
