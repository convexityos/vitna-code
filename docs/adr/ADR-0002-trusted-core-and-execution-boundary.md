# ADR-0002: Trusted Core and Execution Boundary

- Status: Accepted
- Date: 2026-09-16
- Deciders: Vitna Core Engineering

## Context

Coding agents must execute untrusted code, run linters, invoke build tools, and apply source code patches. If untrusted repository code executes within the primary orchestrator process, any buffer overflow, rogue memory access, or environment manipulation can compromise sessions, tamper with receipts, or exfiltrate credentials. Furthermore, processes spawned without a durable execution journal can crash halfway through side effects, leaving the orchestrator unaware of whether mutations succeeded or failed.

## Decision

1. **Three-Process Architectural Separation**:
   - **`vitna-coded` (Daemon)**: Manages sessions, turns, context assembly, provider calls, policy decisions, approvals, and event logging. It listens on an owner-only local socket or named pipe and **never executes untrusted repository code in its own address space**.
   - **`vitna-runner` (Execution Runner)**: A hardened, minimal Rust security boundary. Every untrusted process execution, PTY session, and file mutation runs through `vitna-runner` under OS-enforced sandboxing.
   - **`vitna-git-broker` (Git Broker)**: A dedicated, isolated process handling private repository mirrors, dirty snapshot captures, disposable agent workspace materialization, and final patch integration. The Git broker disables repository hooks, filters, aliases, and dangerous protocols.

2. **Durable Daemon-Runner Action Protocol**:
   All effectful operations between the daemon and runner use a 5-step transactional journal:
   - **Step 1 (Approved)**: Daemon records the approved action and argument digest in the WAL event log.
   - **Step 2 (Prepared)**: Runner durably records `prepared` to its local journal before acknowledging the request to the daemon.
   - **Step 3 (Started)**: Runner transitions to `started` immediately before spawning child processes or applying mutations.
   - **Step 4 (Finished)**: Runner records `finished` along with exit status, stdout/stderr hashes, and result digests.
   - **Step 5 (Acknowledged)**: Daemon persists the final result in its store and acknowledges the runner, releasing retained runner journal entries.

3. **Crash and Ambiguous-State Handling**:
   - If daemon-runner connectivity is lost during an effectful action, the daemon queries the runner by action ID upon reconnection.
   - If neither the daemon nor the runner can prove the final state of an effect, the action transitions to `needs_reconciliation`.
   - **Mutating actions are never retried automatically**.

4. **Runner Execution Statements**:
   - Statements produced by `vitna-runner` are signed with an ephemeral local runner identity and linked into the receipt event chain, ensuring post-finalization tampering is detectable.

## Consequences

### Positive
- Strict process-level isolation protects provider credentials, cryptographic keys, and daemon state from hostile repository code.
- Crash recovery is deterministic: completed effects are acknowledged, and ambiguous effects are flagged for human review rather than silently duplicated.
- Runner can be fuzzed, audited, and verified independently of the full daemon orchestration logic.

### Negative / Trade-offs
- IPC overhead for every command invocation and structured file write (measured at less than 5ms locally, well within the 100ms p95 delivery budget).
- Requires maintaining separate runner binaries across Tier 1 target platforms.
