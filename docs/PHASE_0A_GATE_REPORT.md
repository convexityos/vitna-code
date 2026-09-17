# Phase 0A Gate Report: Vitna Code

- Date: 2026-09-16
- Evaluator: Vitna Core Engineering
- Gate Target: Phase 0A Exit Gate
- Status: **PASSED (Ready for Phase 0B Schemas)**

---

## 1. Executive Summary

Phase 0A establishes the foundational contracts, threat model, authority boundaries, platform isolation guarantees, dependency licenses, and competitor baseline for Vitna Code (`convexityos/vitna-code`).

All Phase 0A criteria have been satisfied without compromising or weakening any Vitna product invariants. Every assertion in this report points directly to an Architectural Decision Record (ADR), specification, repository manifest, or automated test harness.

---

## 2. Gate Verification Checklist

| Phase 0A Gate Requirement | Status | Evidence Reference |
|---|---|---|
| **1. Authoritative Core Ownership**<br>`vitna-coded` is established as the sole owner of session, turn, context, approval, and effect state. Foreign runtimes are explicitly rejected. | **PASS** | See `ADR-0001` (Section 1), `Cargo.toml`, and `crates/daemon/src/lib.rs`. |
| **2. Hardened Effect Boundaries**<br>Every repository mutation and process execution path is routed strictly through `vitna-runner` or `vitna-git-broker`. No untrusted code executes inside the daemon. | **PASS** | See `ADR-0002` (Sections 1-2), `docs/AUTHORITY_INVENTORY.md`, and `crates/vitna-runner/src/lib.rs`. |
| **3. Dependency Governance**<br>Every foundational dependency is audited across 6 criteria (license, Tier 1 support, zero telemetry, no ambient authority, fuzzability, replacement path). | **PASS** | See `docs/DEPENDENCY_MATRIX.md`. All workspace crates pass Apache-2.0 / MIT licensing checks. |
| **4. Threat Model and Boundary Analysis**<br>7 explicit trust boundaries and 6 trust levels defined. All 10 high-risk threat vectors mapped to automated test plans. | **PASS** | See `docs/threat-model/THREAT_MODEL.md` (Sections 2-5). |
| **5. Exhaustive Authority Inventory**<br>All 10 effect classes (reads, writes, Git, process, network, secrets, providers, MCP, updates, policy) completely cataloged. | **PASS** | See `docs/AUTHORITY_INVENTORY.md`. |
| **6. Tier 1 Platform Guarantee Matrix**<br>Real-hardware enforcement mechanisms and proof plans documented for Windows 11 (x64/ARM64), macOS, and Linux. | **PASS** | See `docs/PLATFORM_MATRIX.md` and `.github/workflows/ci.yml`. |
| **7. Competitor Baseline & Claim Mapping**<br>10 coding agents analyzed with pinned versions as of 2026-09-16, primary documentation citations, and structural openings identified. | **PASS** | See `docs/COMPETITOR_DOSSIER.md`. |
| **8. Invariant Preservation**<br>Zero cloud request-path dependencies; customer-held credentials; no code exfiltration; no automatic retry of ambiguous effects; zero em-dashes. | **PASS** | See `IMPLEMENTATION_STATUS.md` and `scripts/dev.ps1`. |

---

## 3. Detailed Findings and Evidence

### 3.1 Architecture and Process Separation (ADR-0001, ADR-0002)
- The architecture cleanly separates `vitna-coded` (orchestrator), `vitna-runner` (sandbox execution), and `vitna-git-broker` (isolated Git mirror and workspace management).
- Untrusted code execution in the daemon address space is architecturally barred.
- The 5-step action journal (`approved` -> `prepared` -> `started` -> `finished` -> `acknowledged`) prevents duplicate side effects and forces ambiguous mutations into `needs_reconciliation`.

### 3.2 Data Retention and Rest Storage (ADR-0003, ADR-0004)
- Local SQLite database and Write-Ahead Logs are protected against plaintext storage of sensitive code and tool arguments through per-user Data Encryption Keys (DEKs) wrapped by OS keychains.
- SQLite access is strictly serialized through a single-writer `DatabaseActor` to eliminate lock contention.
- Artifact storage uses a two-phase atomic write-sync-rename commit protocol.

### 3.3 Wire Framing and Local IPC (ADR-0005)
- Local IPC uses owner-only Unix domain sockets (mode `0600`) or Windows Named Pipes with strict User SID DACLs.
- Default loopback TCP listeners are forbidden, preventing browser-based cross-origin attacks.
- Binary length-prefixed Protobuf envelopes provide monotonic sequence tracking and version negotiation.

### 3.4 Platform Parity and Windows First-Class Support (PLATFORM_MATRIX.md)
- Windows 11 x64 and ARM64 are established as equal release targets alongside Linux and macOS.
- Sandbox mechanisms map to AppContainer and Restricted Tokens on Windows, Bubblewrap and Landlock on Linux, and native sandbox profiles on macOS.

---

## 4. Recommendation and Next Steps

Phase 0A is **officially closed and approved**.

We are cleared to proceed to **Phase 0B**, which will formalize:
1. Complete Protobuf schemas for the Vitna Agent Protocol commands and events (`crates/protocol`).
2. SQLite table definitions, migrations, and event replay state machine (`crates/store`).
3. Runner journal serialization format and recovery queries (`crates/vitna-runner`).
4. Cryptographic receipt hash-chain specification and golden test vectors (`crates/receipts`).
