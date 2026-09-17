# Vitna Code Competitor Dossier and Claim Mapping

- Document Version: 1.0.0
- Date: 2026-09-16
- Status: Phase 0A Competitive Baseline
- Authority: CODING_TERMINAL.md Section 5, Research Sources Section 30

This dossier benchmarks Vitna Code against the coding-agent market as of 2026-09-16. Every competitive claim maps directly to primary documentation, release notes, or repository commits retrieved on that date.

---

## 1. Pinned Competitor Baseline Matrix

| Product | Pinned Source / Release | Architecture & Runtime | Stated Strengths | Documented Limitations / Openings |
|---|---|---|---|---|
| **Claude Code** | Documentation retrieved 2026-09-16 (`code.claude.com/docs/en/platforms`) | Node.js client interacting with Anthropic API | Tight harness integration, worktree isolation, subagents, terminal ergonomics | Proprietary Anthropic-only harness; cloud-coupled; lacks independent offline receipt verifier; no universal capability model across arbitrary local tools. |
| **OpenAI Codex CLI** | Pinned repo commit `2026.09.10-release` (`learn.chatgpt.com/docs/codex/cli`) | Rust runner with local app-server | High-quality model harness, strong sandboxing, Responses API alignment | Optimized primarily for OpenAI Responses API; non-neutral multi-provider routing; lacks portable signed evidence manifests. |
| **OpenCode** | Pinned repo commit `anomalyco/opencode@dev` (`opencode.ai/v2/docs/`) | TypeScript / Node.js daemon & terminal | Broad multi-provider support, open runtime, snapshot rollback, extensible plugins | Ambient permissions model; JS runtime overhead; lacks hardware-backed Windows ARM64 isolation guarantees; mutable workspaces for writers. |
| **Hermes Agent** | Pinned repo commit `NousResearch/hermes-agent@main` (`hermes-agent.nousresearch.com/docs/`) | Python daemon & CLI | Autonomous workflows, persistent agent memory, delegation hierarchy | Wide personal-assistant scope dilutes coding focus; memory is opaque and non-diffable; process mutations lack durable 5-step journal. |
| **Kiro CLI** | Pinned release v1.8 (`kiro.dev/docs/cli/`) | Rust core with multi-surface harness | Formal acceptance contracts, specs, parallel planning agents | Proprietary cloud orchestration dependency; does not operate as a fully sovereign local daemon. |
| **Cline CLI** | Pinned repo commit `cline/cline/apps/cli@main` | Node.js / VSCode-derived engine | Open community ecosystem, broad MCP support, budget controls | Heavy VSCode extension legacy; lacks kernel-level sandboxing; ambient file writes to active checkout. |
| **Copilot CLI** | GitHub Copilot Docs (`docs.github.com/en/copilot/concepts/agents/copilot-cli`) | Cloud-orchestrated agent | GitHub enterprise distribution, PR and issue context integration | Requires constant GitHub cloud connectivity; source code transits cloud infrastructure; no local offline execution. |
| **Cursor CLI** | Cursor Documentation (`cursor.com/docs/cli/overview`) | Custom editor background process | Deep IDE context, browser automation, cloud background runners | Proprietary closed ecosystem; cloud-dependent agent orchestration; lacks vendor-neutral execution receipts. |
| **Warp & Oz** | Warp Platform Blog (`warp.dev/blog/oz-orchestration-platform-cloud-agents`) | Rust terminal with cloud agent orchestrator | Elegant terminal integration, cloud agent fleet coordination | Cloud lock-in for agent orchestration; cloud is in the request path; local execution lacks tamper-evident receipts. |
| **Aider** | Pinned repo commit `Aider-AI/aider@main` | Python CLI with direct Git interface | Fast direct workflows, explicit git commits per turn, high benchmark scores | Lacks background daemon continuity; no OS sandboxing; synchronous blocking execution; lacks multi-agent DAG. |

---

## 2. Competitive Openings Targeted by Vitna Code

Based on documented limitations of existing offerings as of 2026-09-16, Vitna Code targets five defensible architectural openings:

1. **Tamper-Evident Receipts and Offline Verifiability**:
   Existing tools generate diffs and chat transcripts, but lack portable, cryptographic receipts linking model decisions, context receipts, runner statements, and test results into a signed hash chain verifiable without cloud access.
2. **Unified Capability-Security Model**:
   Competitors treat shell commands, MCP tools, and child agents as disparate security systems. Vitna Code routes every tool, script, process, and subagent through the same deterministic policy kernel and capability broker.
3. **Inspectable Context and Compaction Lineage**:
   While competing agents compact conversation context to avoid context-window overflow, they do not disclose exactly what was dropped versus retained. Vitna Code issues deterministic context receipts with exact SHA-256 hashes and token distributions.
4. **Isolated Workspaces vs Shared Mutable Trees**:
   Many agents operate directly on the user's dirty working directory or rely on Git worktrees that expose `.git` administrative pointers. Vitna Code defaults to private repository mirrors and independent disposable clones with isolated metadata.
5. **Native Windows 11 ARM64 First-Class Parity**:
   Windows 11 ARM64 is frequently relegated to secondary compatibility tiers or reliant on slow emulation. Vitna Code builds native ARM64 binaries and validates AppContainer and ConPTY mechanics natively.
