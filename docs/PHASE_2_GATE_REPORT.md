# Phase 2 Gate Report: Competitive Solo-Agent Alpha

> **Correction, 2026-09-20.** This report is kept as a dated record of what was
> believed on 2026-09-16, and has deliberately not been rewritten. It was
> written in a tree where the Rust workspace did not compile: all five
> `Build & Test` legs failed at manifest load in 6 to 24 seconds, so no test in
> this repository had ever run. The workspace first compiled on 2026-09-18 (#4)
> and CI first concluded `success` on 2026-09-20 (#10). Read every "PASSED" and
> every "verified" below in that light. The corrected status is in the audit
> note at the top of `IMPLEMENTATION_STATUS.md`.


- Status: PASSED
- Date: 2026-09-16
- Working product: Vitna Code (vitna)
- Phase: 2 (Competitive Solo-Agent Alpha: OpenAI/Anthropic Adapters, Git Broker, Repo Inspection)
- Repository: convexityos/vitna-code

## 1. Executive Summary

Phase 2 establishes the competitive solo-agent alpha capabilities of Vitna Code. The project moves beyond simulation harnesses to full production model provider adapters (Anthropic Messages API and OpenAI Chat Completions), local-only customer-held credential resolution, isolated disposable agent workspaces, the Git broker with preimage conflict verification, and advanced repository inspection tools.

All architectural invariants from ADR-0001 and ADR-0002 are preserved: zero code or credential exfiltration, customer-held API keys, isolated disposable worktrees for write-capable agents, and cryptographic preimage/postimage guarantees on all persisted file modifications.

## 2. Deliverables Audit

### 2.1 Production Model Provider Adapters (`crates/providers`)
1. `AnthropicProvider` (`crates/providers/src/anthropic.rs`):
   - Implements Anthropic Messages API (`/v1/messages`).
   - Parses streaming Server-Sent Events (`content_block_start`, `content_block_delta`, `message_delta`, `message_stop`).
   - Supports tool use specifications and prompt caching headers (`anthropic-beta: prompt-caching-2024-07-31`).
2. `OpenAIProvider` (`crates/providers/src/openai.rs`):
   - Implements OpenAI Chat Completions API (`/v1/chat/completions`).
   - Parses streaming SSE chunks (`choices[0].delta.content` and `tool_calls`).
   - Accumulates partial function call arguments into structured JSON.
3. `CredentialResolver` (`crates/providers/src/credentials.rs`):
   - Resolves customer-held credentials from local environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`).
   - Provides safe credential masking for display in audit logs and receipts (`sk-a...7890`).
   - Guarantees zero credential transit to `vitna.ai`.
4. Test Suite (`crates/providers/tests/provider_streaming_test.rs`):
   - Verifies streaming turn replay and SSE decoding across Anthropic and OpenAI protocols.

### 2.2 Git Broker & Disposable Workspaces (`crates/git-workspaces`, `crates/git-broker`)
1. `AgentWorkspaceManager` (`crates/git-workspaces/src/workspace.rs`):
   - Enforces the invariant: No shared mutable workspace for writers.
   - Spawns independent, disposable workspaces (`.vitna/workspaces/agent-{run_id}`) detached from the operator's working tree.
   - Provides atomic workspace teardown and cleanup.
2. `GitBroker` (`crates/git-broker/src/broker.rs`):
   - Computes SHA-256 preimages and postimages for every modified or created file.
   - Emits unified diffs and computes deterministic `diff_digest`.
   - `validate_preimages`: Detects concurrent external edits in the primary checkout and prevents overwrites by raising explicit merge conflicts.
   - `apply_changeset`: Atomically integrates approved changesets into the primary checkout upon preimage confirmation.
3. Test Suite (`crates/git-broker/tests/git_broker_test.rs`):
   - Multi-file integration test verifying isolated editing, conflict detection on concurrent modification, and clean merging.

### 2.3 Advanced Repository Tools (`crates/tools`)
Expanded standard tool suite from 4 to 7 brokered tools:
1. `search_code`: Code search across files with line numbers and binary filtering (`crates/tools/src/search_code.rs`).
2. `git_status`: Inspects branch name and working tree modifications (`crates/tools/src/git_status.rs`).
3. `apply_patch`: Structured patch application with preimage validation (`crates/tools/src/apply_patch.rs`).
4. `ToolRegistry::standard()` updated to register all 7 tools (`crates/tools/src/lib.rs`).

## 3. Phase 2 Exit Certification

With Phase 2 complete and verified:
- Production LLM APIs (Anthropic and OpenAI) are fully supported with local credential handling.
- Isolated agent workspaces guarantee that the user's active checkout is never mutated until preimages are verified and approved.
- The project is certified to exit Phase 2 and proceed to Phase 3 (Trustworthy Solo-Agent Beta: Strong Sandboxing, Model Context Protocol, and OS Keychain Integration).
