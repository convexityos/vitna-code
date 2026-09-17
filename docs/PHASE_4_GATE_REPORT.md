# Phase 4 Gate Report: Durable Multi-Agent Beta

- Status: PASSED
- Date: 2026-09-16
- Working product: Vitna Code (vitna)
- Phase: 4 (Durable Multi-Agent Beta: DAG Scheduler, Per-Agent Clones, Merge Queue, Receipt Hierarchy)
- Repository: convexityos/vitna-code

## 1. Executive Summary

Phase 4 expands Vitna Code from solo-agent execution to durable multi-agent orchestration without compromising any safety or cryptographic invariants. Multi-agent workflows in Vitna Code are coordinated through a deterministic directed acyclic graph (DAG) task scheduler with Kahn's algorithm cycle detection and cascading failure cancellation.

To uphold the core invariant that no two agents ever write to the same mutable working tree, each child agent operates inside a disposable branch clone (`AgentWorkspace`). Changes from concurrent child agents are committed to a serialized `MergeQueue` that re-validates cryptographic preimages against the primary checkout immediately prior to application. If a sibling agent has mutated a target file, the conflict is flagged explicitly rather than silently overwritten. Finally, child agent execution evidence and Merkle roots are aggregated into a signed parent coordinator receipt (`VitnaRunReceiptV1`).

## 2. Deliverables Audit

### 2.1 DAG Task Scheduler (`crates/orchestration/src/dag.rs`)
1. Data Structures:
   - `TaskStatus`: `Pending`, `Running`, `Succeeded`, `Failed`, `Cancelled`.
   - `TaskNode`: Task identity, role assignment, prompt specification, dependencies, execution run ID, and output summary.
   - `TaskGraph`: Validated task collection and dependency graph.
2. Topological Sort and Cycle Detection:
   - Evaluates dependency existence and detects circular dependencies via Kahn's algorithm.
3. State Transitions and Cascading Cancellation:
   - `get_ready_tasks()`: Identifies pending tasks whose prerequisite dependencies have all succeeded.
   - `mark_failed()`: Automatically cascades cancellation to all transitive downstream dependencies.
   - `is_complete()`: Confirms when all graph nodes have reached terminal states.

### 2.2 Serialized Multi-Agent Merge Queue (`crates/git-broker/src/merge_queue.rs`)
1. Data Structures:
   - `MergeQueueItem`: Queue item tracking task ID, isolated agent workspace path, and computed `ChangeSet`.
   - `MergeQueueResult`: `Merged` (with applied files list), `Conflict` (with detailed preimage conflict list), or `Error`.
   - `MergeQueue`: Manages pending FIFO queue and historical merge records.
2. Safe Sequential Application:
   - Re-evaluates cryptographic preimages against the primary repository at merge time.
   - Rejects concurrent sibling overwrites with explicit conflict details.
   - Exports: `MergeQueue`, `MergeQueueItem`, and `MergeQueueResult` in `crates/git-broker/src/lib.rs`.

### 2.3 Hierarchical Receipt Aggregation (`crates/receipts`)
1. Schema & Data Model Updates:
   - Updated `schemas/vitna-run-receipt-v1.json` to include optional `child_receipt_roots`.
   - Added `child_receipt_roots: Vec<String>` to `VitnaRunReceiptV1` with `skip_serializing_if = "Vec::is_empty"` to preserve backwards compatibility.
2. Aggregation Helper:
   - `aggregate_child_receipt(&mut self, child_receipt: &VitnaRunReceiptV1)` collects child event hash chain roots and deduplicates evidence items.
   - Canonical RFC 8785 byte generation and Ed25519 signature verification cover child receipt roots.

### 2.4 Multi-Agent Integration Test (`crates/orchestration/tests/multi_agent_dag_test.rs`)
1. 3-Agent Pipeline:
   - Stage 1: Researcher agent inspects primary repo, outputs findings, produces receipt.
   - Stage 2: Backend agent creates backend service in disposable clone, calculates preimages/postimages, submits to `MergeQueue`, merges cleanly.
   - Stage 3: Tester agent runs verification suite against merged output, produces test receipt.
2. Aggregation & Verification:
   - Coordinator combines child receipts into parent receipt.
   - Signs parent receipt with coordinator device key.
   - Validates via offline `ReceiptVerifier` with zero errors.

## 3. Phase 4 Exit Certification

With Phase 4 complete and verified:
- Multi-agent coordination is deterministic, acyclic, and failure-cascading.
- Child agents never share mutable workspaces.
- Merge queue prevents silent overwrite anomalies between concurrent agents.
- Receipts form a complete cryptographic hierarchy from parent to subagents.
- The project is certified to exit Phase 4 and proceed to Phase 5 (V1 Hardening and Release: Packaging, Release Manifests, Signed Installers, SBOM, Benchmark Suite).
