# ADR-0004: Persistence Engine and Actor Model

- Status: Accepted
- Date: 2026-09-16
- Deciders: Vitna Core Engineering

## Context

Vitna Code requires high-throughput, low-latency event persistence to record every model chunk, tool event, approval, and state transition. The system must survive daemon crashes, power loss, and system reboots without database corruption, lost events, or inconsistent run state.

Traditional multi-threaded database connections often suffer from lock contention in SQLite (`SQLITE_BUSY`), unpredictable transaction timeouts, and race conditions between concurrent readers and writers.

## Decision

1. **SQLite in WAL Mode with Single-Writer Actor**:
   - Use SQLite with Write-Ahead Logging (`PRAGMA journal_mode = WAL;`) and synchronous normal (`PRAGMA synchronous = NORMAL;`).
   - All write operations are serialized through a single dedicated Tokio actor task (`DatabaseActor`) receiving commands over a bounded MPSC channel.
   - Read queries can execute concurrently using a pooled set of read-only SQLite connections (`PRAGMA query_only = ON;`).

2. **Rusqlite with Bundled C Source**:
   - Use the `rusqlite` crate with the `bundled` feature flag enabled.
   - Bundling SQLite C source ensures consistent, reproducible behavior across Windows 11 (x64 and ARM64), macOS, and Linux without depending on host OS dynamic libraries or system SQLite version mismatches.

3. **Hybrid Persistence Model**:
   - **Append-Only Event Store**: Stores raw, monotonic, cryptographic event log rows (`event_id`, `run_id`, `sequence`, `type`, `encrypted_payload`, `prev_hash`, `event_hash`). Events are immutable and never updated or deleted during normal runs.
   - **Materialized State Tables**: Transactionally updated projections for active entities (`sessions`, `turns`, `runs`, `approvals`, `artifacts`, `leases`) to support sub-millisecond query performance for UI rendering and status checks.
   - State can always be reconstructed by replaying the append-only event stream from sequence zero.

4. **Two-Phase Artifact Commit Sequence**:
   To prevent dangling database references or orphaned files:
   - Step 1: Reserve artifact storage quota.
   - Step 2: Write encrypted payload to a temporary file in the state directory.
   - Step 3: Compute SHA-256 digest and flush file buffers (`fsync`).
   - Step 4: Atomically rename file into content-addressed artifact store (`artifacts/<hash>`).
   - Step 5: Commit SQLite artifact record and corresponding event in one database transaction.
   - Step 6: Acknowledge the producing runner or tool broker.

## Consequences

### Positive
- Zero `SQLITE_BUSY` errors because all writes pass through the single-writer channel.
- Complete crash recovery: replaying the event log yields the exact same materialized state.
- Highly performant concurrent reads for the TUI, CLI, and external verifier tools.

### Negative / Trade-offs
- Actor architecture requires asynchronous message passing for write transactions.
- Periodic WAL checkpointing (`PRAGMA wal_checkpoint(TRUNCATE)`) must be managed during idle daemon windows.
