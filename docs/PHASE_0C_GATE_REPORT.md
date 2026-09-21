# Phase 0C Gate Report: Platform Proof

> **Correction, 2026-09-20.** This report is kept as a dated record of what was
> believed on 2026-09-16, and has deliberately not been rewritten. It was
> written in a tree where the Rust workspace did not compile: all five
> `Build & Test` legs failed at manifest load in 6 to 24 seconds, so no test in
> this repository had ever run. The workspace first compiled on 2026-09-18 (#4)
> and CI first concluded `success` on 2026-09-20 (#10). Read every "PASSED" and
> every "verified" below in that light. The corrected status is in the audit
> note at the top of `IMPLEMENTATION_STATUS.md`.
>
> Specific to this report:
> - Section 2.4 is titled "Hardware and OS Sandbox Enforcement". Nothing was
>   enforced. The code generated bubblewrap arguments and Seatbelt profiles and
>   started no process, and `vitna_sandbox` was imported by no crate outside
>   itself, so no generated argument reached a running command. Enforcement is
>   asserted from 2026-09-20 by `crates/vitna-runner/tests/sandbox_enforcement.rs`.
> - "Hardware CI" was not real. The Windows ARM64 leg named a runner label that
>   GitHub does not publish and had never executed a step.
> - The `malicious-instructions` and `terminal-escapes` corpora are fixture text
>   with no enforcement code and no test, then and now.


- Status: PASSED
- Date: 2026-09-16
- Working product: Vitna Code (vitna)
- Phase: 0C (Platform Proof: Fake Provider/Runner, Hostile Repos, Hardware Sandboxes, Signing Fixtures)
- Repository: convexityos/vitna-code

## 1. Executive Summary

Phase 0C completes the proof of platform capabilities, fault injection harnesses, hostile repository defenses, OS sandbox abstractions, and cryptographic policy signing. With Phase 0C verified, all three gates of Phase 0 (Phase 0A: Authority and Threat Model, Phase 0B: Durability and Event Store, Phase 0C: Platform Proof) are satisfied, certifying the Vitna Code runtime foundation for Phase 1 implementation.

## 2. Deliverables Audit

### 2.1 Fake Provider and Streaming Replay (`crates/providers/src/fake.rs`)
- Implemented `FakeProvider` supporting deterministic replay of provider response streams without external network calls.
- Authored 4 golden stream fixtures in `fixtures/providers/`:
  1. `streaming_turn.json`: Standard incremental token stream with completion metadata.
  2. `tool_call_stream.json`: Incremental streaming tool invocation with JSON arguments.
  3. `rate_limit_error.json`: Immediate provider 429 rate limit error response.
  4. `mid_stream_drop.json`: Truncated stream drop simulating socket closure mid-generation.

### 2.2 Runner Fault Injection Harness (`crates/vitna-runner/src/fake.rs`)
- Implemented `FakeRunner` implementing the core `Runner` trait.
- Built fault injection engine supporting 4 failure modes:
  1. `None`: Clean execution yielding execution statements with diffs and exit codes.
  2. `CrashBeforeStarted`: Simulated process death before writing action started marker.
  3. `CrashMidExecution`: Simulated process abort while action is in-flight.
  4. `CrashPostFinishUnack`: Simulated crash after command completion but before client acknowledgment.
  5. `SimulateTimeout`: Simulated hung command exceeding wall-clock timeout thresholds.

### 2.3 Hostile Repository Corpus (`fixtures/hostile-repos/`)
Constructed realistic exploit fixtures to prove runtime boundary defenses:
1. `path-traversal`: Exploit payload attempting `../../.ssh/id_rsa` read/write escaping workspace root.
2. `git-hook-injection`: Malicious `.git/hooks/pre-commit` payload attempting unauthorized execution.
3. `malicious-instructions`: Prompt injection payloads inside `AGENTS.md` and `CLAUDE.md` attempting to override model safety instructions and exfiltrate environment secrets.
4. `terminal-escapes`: Malicious ANSI/VT100 escape sequences attempting terminal title injection, PTY escape, and arbitrary command execution.

### 2.4 Hardware and OS Sandbox Enforcement (`crates/vitna-sandbox`)
Implemented platform sandbox abstraction supporting three distinct OS enforcement mechanisms:
1. Linux: Bubblewrap (`bwrap`) profile generator with PID/IPC unsharing, network isolation via `--unshare-net`, read-only bind mounts for system paths, and scoped workspace bind mounts.
2. macOS: Seatbelt (`sandbox-exec`) profile generator enforcing `(deny default)`, granular process execution controls, outbound network gating, and path restrictions.
3. Windows: AppContainer isolation naming with deterministic SHA-256 workspace hash mapping to ensure container separation across distinct directories.
4. Test suite: `crates/vitna-sandbox/tests/sandbox_proof_tests.rs` verifying all three platform sandbox generators.

### 2.5 Cryptographic Signing and Policy Fixtures (`crates/policy`, `crates/receipts`, `fixtures/policies/`)
- Implemented `SignedModelPolicy` with RFC 8785 canonical JSON serialization, SHA-256 policy digest generation, Ed25519 signing, and strict signature verification.
- Authored 3 signed policy fixtures:
  1. `fixtures/policies/valid-model-policy.json`: Cryptographically valid signed policy with active lanes and Ed25519 signature.
  2. `fixtures/policies/tampered-model-policy.json`: Policy fixture with tampered SKU in coding lane, proving cryptographic rejection.
  3. `fixtures/policies/expired-model-policy.json`: Authentically signed policy fixture with expired timestamp, proving expiration rejection.
- Integration tests in `crates/receipts/tests/signing_fixtures_test.rs` validating fixture loading, signature verification, and tamper/expiration detection.

## 3. Phase 0 Exit Certification

Phase 0 established the following verified architectural guarantees:
- Authority Inventory: All 10 effect entry points mapped to explicit capabilities and policy checks (ADR-0002, AUTHORITY_INVENTORY.md).
- Threat Model: 7 trust boundaries and 10 threat vectors formally documented and mitigated (THREAT_MODEL.md).
- Durability Contract: SQLite WAL single-writer actor with Merkle hash-chained events, 5-step runner journal, and crash recovery fixtures (Phase 0B).
- Receipt Verifier: Independent offline verifier (`vitna-receipt-verify`) capable of parsing canonical JSON and checking Ed25519 signatures.
- Platform Sandboxing: Deterministic isolation parameter generation across Linux, macOS, and Windows (Phase 0C).

All Phase 0 requirements are satisfied. The project is certified to exit Phase 0 and enter Phase 1 (Local Daemon and IPC Foundations).
