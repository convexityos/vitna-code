# Phase 0B Gate Report: Vitna Code (Durability & Schemas)

- Date: 2026-09-16
- Evaluator: Vitna Core Engineering
- Gate Target: Phase 0B Exit Gate
- Status: **PASSED (Ready for Phase 0C Platform Proof)**

---

## 1. Executive Summary

Phase 0B formalizes the durable schemas, wire framing, transaction journal, and cryptographic trust models for Vitna Code (`convexityos/vitna-code`).

All Phase 0B criteria have been successfully satisfied without compromising any architectural invariants:
- The wire protocol schemas are frozen under `protocol/vitna/protocol/v1/`.
- The append-only event store schema and initial migration (`0001_initial_schema.sql`) are implemented and tested in `crates/store/`.
- The durable 5-step runner action journal (`prepared` -> `started` -> `finished` -> `acknowledged`) is implemented in `crates/vitna-runner/`.
- The cryptographic receipt model (`vitna-run-receipt-v1`), canonical JSON serializer (RFC 8785), Merkle tree generator, and offline verification engine are delivered in `crates/receipts/` and `crates/vitna-receipt-verify/`.
- Golden crash recovery fixtures in `fixtures/crash-recovery/` validate that ambiguous mutations are forced into `needs_reconciliation` and never automatically retried.

---

## 2. Gate Verification Checklist

| Phase 0B Gate Requirement | Status | Evidence Reference |
|---|---|---|
| **1. Protocol Schemas & Framing**<br>Protobuf definitions for commands, events, and envelopes frozen with 4-byte big-endian framing and 16MB size limits. | **PASS** | See `protocol/vitna/protocol/v1/` (`commands.proto`, `events.proto`, `envelope.proto`) and `crates/protocol/src/lib.rs`. |
| **2. Append-Only Event Store & Migrations**<br>Cryptographic SHA-256 event hash chaining and initial SQLite WAL migration with materialized entities. | **PASS** | See `crates/store/migrations/0001_initial_schema.sql` and `crates/store/src/lib.rs`. |
| **3. Durable Runner Action Journal**<br>5-step action protocol (`prepared` -> `started` -> `finished` -> `acknowledged`) with disk-backed persistence and recovery query. | **PASS** | See `crates/vitna-runner/src/journal.rs` and unit tests. |
| **4. Receipt Trust Model & Offline Verifier**<br>RFC 8785 canonical JSON serialization, Merkle tree root calculation, Ed25519 signing, and offline verifier CLI. | **PASS** | See `schemas/vitna-run-receipt-v1.json`, `crates/receipts/src/lib.rs`, and `crates/vitna-receipt-verify/src/lib.rs`. |
| **5. Crash Recovery Fixtures**<br>Golden event streams simulating clean completion, crash mid-tool execution, and crash post-finish unacknowledged. | **PASS** | See `fixtures/crash-recovery/` and `crates/store/tests/replay_tests.rs`. |
| **6. Non-Negotiable Invariants**<br>No automatic retries of ambiguous mutating effects; zero code exfiltration; zero em-dashes across all files. | **PASS** | Verified across all test suites and automated em-dash scans. |

---

## 3. Technical Audit Findings

### 3.1 Wire Protocol Framing (`crates/protocol`)
- Big-endian 4-byte length prefix ensures frame boundary enforcement.
- Strict 16MB size ceiling (`MAX_FRAME_SIZE_BYTES`) protects the daemon against memory exhaustion attacks over local IPC.
- `HandshakeRequest` and `HandshakeResponse` negotiate version ranges (major/minor) to ensure backward compatibility across client updates.

### 3.2 Event Store & Hash Chain (`crates/store`)
- Each event row records `prev_event_hash` and computes `event_hash = SHA256(prev_hash : run_id : sequence : type_url : timestamp_ms : payload_hash)`.
- Replaying events from sequence zero validates hash continuity; any tampering immediately transitions run state to `needs_reconciliation`.
- SQLite operates in WAL mode with foreign key checks enabled and serialized write access via single-writer actor architecture.

### 3.3 Runner Journal Protocol (`crates/vitna-runner`)
- The 5-step action protocol guarantees that runner disk persistence precedes subprocess execution.
- If a runner or host crashes while an action is in `started` or `prepared` state, `reconcile_crashed_actions()` flags the action as `needs_reconciliation`.
- Replay tests confirm that ambiguous side effects are never retried automatically.

### 3.4 Cryptographic Receipts (`crates/receipts` & `crates/vitna-receipt-verify`)
- `VitnaRunReceiptV1` serializes to deterministic RFC 8785 canonical JSON format, eliminating signature invalidation due to key reordering or whitespace formatting differences.
- Merkle root computation over event hashes proves log completeness.
- Offline verifier (`vitna-receipt-verify`) validates receipts independently without daemon, database, or network access.

---

## 4. Recommendation and Next Steps

Phase 0B is **officially closed and approved**.

We are cleared to proceed to **Phase 0C (Platform Proof)**, which will deliver:
1. Fake provider adapter and golden streaming test harness (`crates/providers`).
2. Fake runner implementation with fault injection points (`crates/vitna-runner`).
3. Hostile repository security corpus (symlinks, path traversal, git hook escapes) in `fixtures/hostile-repos/`.
4. Real-hardware sandbox test proofs for Windows 11 (x64/ARM64), macOS, and Linux.
5. Multi-platform CI pipeline validation.
