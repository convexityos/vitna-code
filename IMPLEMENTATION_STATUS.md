# Vitna Code Implementation Status

- Status: Phase 0A Complete
- Date: 2026-09-16
- Tracking Mode: Evidence-backed milestones (no percentage estimates)

## Phase Gates Overview

| Phase | Description | Status | Evidence / Verification Gate |
|---|---|---|---|
| **Phase 0A** | Authority, Invariants, Threat Model, Platform & Dependency Matrices | **COMPLETE** | See `docs/PHASE_0A_GATE_REPORT.md` |
| **Phase 0B** | Protocol Envelope, Event Store, Runner Journal, Receipt Trust Model | NOT STARTED | Blocked on Phase 0A user review |
| **Phase 0C** | Platform Proof: Fake Provider & Runner, Hostile Repos, Hardware CI | NOT STARTED | Blocked on Phase 0B completion |
| **Phase 1** | Durable Vertical Slice (CLI, TUI, Daemon, Guarded Runner, Local Receipt v0) | NOT STARTED | Blocked on Phase 0 exit |
| **Phase 2** | Competitive Solo-Agent Alpha (OpenAI/Anthropic adapters, Git broker, inspect/build) | NOT STARTED | Blocked on Phase 1 exit |
| **Phase 3** | Trustworthy Solo-Agent Beta (Strong sandbox, MCP, keychain secrets, Playwright) | NOT STARTED | Blocked on Phase 2 exit |
| **Phase 4** | Durable Multi-Agent Beta (DAG scheduler, per-agent clones, merge queue) | NOT STARTED | Blocked on Phase 3 exit |
| **Phase 5** | V1 Hardening and Release (Packaging, signed installers, SBOM, public benchmarks) | NOT STARTED | Blocked on Phase 4 exit |

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
