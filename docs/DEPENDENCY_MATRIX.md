# Vitna Code Dependency and License Decision Matrix

- Document Version: 1.0.0
- Date: 2026-09-16
- Status: Phase 0A Audit
- Authority: ADR-0001, ADR-0002

Every third-party dependency included in Vitna Code must be vetted against six non-negotiable criteria before inclusion in the workspace:

1. **License & Provenance**: Permissive open-source license (Apache-2.0, MIT, BSD-2/3-Clause, ISC). Zero copyleft (GPL, AGPL, LGPL).
2. **Cross-Platform Parity**: First-class support for Windows 11 x64, Windows 11 ARM64, macOS (x64/ARM64), and Linux (x64/ARM64).
3. **Zero Hidden Telemetry**: No default network egress, phoning home, analytics, or required cloud services.
4. **No Ambient Authority**: Does not bypass declared capability boundaries, scan host filesystems, or extract environment secrets.
5. **Fuzz & Conformance Path**: Parser and serialization components must support property-based testing and fuzzing.
6. **Documented Replacement Path**: Clear architectural alternative available should upstream maintenance lapse.

---

## Workspace Foundation Dependencies Evaluation

| Dependency | Version | License | Target Tier 1 Parity | Zero Telemetry | No Ambient Authority | Fuzz / Conformance | Documented Replacement Path | Decision |
|---|---|---|---|---|---|---|---|---|
| **tokio** | 1.40 | MIT | Windows x64/ARM64, macOS, Linux verified | Verified clean | Uses OS epoll, kqueue, IOCP | Tested via loom and proptest | `async-std` or `smol` | **Approved** |
| **rusqlite** | 0.32 | MIT | Bundled C SQLite compiles across all Tier 1 | Verified clean | Scoped to opened DB path | SQL logic tests and fuzzing | Raw `sqlite3-sys` bindings | **Approved** |
| **prost** | 0.13 | Apache-2.0 | Pure Rust Protobuf runtime, all platforms | Verified clean | In-memory serialization only | Protobuf conformance suite | `quick-protobuf` | **Approved** |
| **serde** / **serde_json** | 1.0 | MIT / Apache-2.0 | Pure Rust data serialization, universal | Verified clean | In-memory serialization only | Comprehensive fuzzing corpus | `simd-json` or `miniserde` | **Approved** |
| **ring** | 0.17 | OpenSSL / ISC / MIT | Native x64 and ARM64 assembly with C fallbacks | Verified clean | In-memory cryptography only | BoringSSL test vectors | `aws-lc-rs` or `dalek` | **Approved** |
| **ed25519-dalek** | 2.1 | BSD-3-Clause | Pure Rust with SIMD optimizations | Verified clean | Pure math over memory buffers | RFC 8032 test vectors | `ring::signature::ED25519` | **Approved** |
| **ratatui** | 0.28 | MIT | Pure Rust terminal layout engine | Verified clean | In-memory buffer manipulation | Layout property tests | Direct ANSI escape formatting | **Approved** |
| **crossterm** | 0.28 | MIT | Cross-platform terminal control (Win32 Console & ANSI) | Verified clean | Bounded to attached stdout/stdin | Terminal escape conformance | `termion` (Unix) + `winconsole` | **Approved** |
| **portable-pty** | 0.8 | MIT | ConPTY on Windows; openpty on Unix | Verified clean | Bounded to runner child processes | PTY session integration tests | Platform-native Win32 / POSIX PTY | **Approved** |
| **rustls** | 0.23 | Apache-2.0 / ISC / MIT | Modern TLS stack without OpenSSL dependencies | Verified clean | Explicit connection configuration | Wycheproof test suite | `native-tls` with OS backend | **Approved** |
| **sha2** / **hex** | 0.10 / 0.4 | MIT / Apache-2.0 | Pure Rust SHA-256 implementation | Verified clean | Memory-only hashing | NIST FIPS 180-4 test vectors | `ring::digest::SHA256` | **Approved** |
| **tracing** | 0.1 | MIT | Structured logging without hidden writers | Verified clean | No automatic disk/net output | In-memory mock subscriber | Custom structured logger | **Approved** |
| **clap** | 4.5 | MIT / Apache-2.0 | CLI argument parsing across all platforms | Verified clean | Evaluates explicitly passed args | Parser property tests | `lexopt` or `argh` | **Approved** |

---

## Prohibited Dependencies and Architectural Rejections

1. **Foreign Agent Runtimes**: Embedding OpenCode daemon, Codex CLI core, or Hermes agent runtime is rejected (ADR-0001).
2. **libgit2 / git2-rs**: Direct integration as the behavioral authority is rejected. Installed Git CLI in a sanitized environment is mandated to prevent behavioral divergences on complex Git features, submodules, and worktrees.
3. **Heavy Browser Automators in Daemon**: Direct embedding of Chromium or Puppeteer in `vitna-coded` is rejected. Narrow headless web verification in Phase 3 is executed out-of-process via `vitna-runner`.
4. **Vector Database / Embeddings Engines**: Local vector databases (e.g., LanceDB, Qdrant embedded) are rejected for Phase 0 and Phase 1 until formal retrieval evals prove measurable improvements over Git, ripgrep, and AST symbol indexes.
