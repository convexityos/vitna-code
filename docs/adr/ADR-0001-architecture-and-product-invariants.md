# ADR-0001: Architecture, Product Invariants, and Authoritative Rust Core

- Status: Accepted
- Date: 2026-09-16
- Deciders: Vitna Core Engineering

## Context

Vitna Code is a local-first coding terminal designed to deliver verifiable, evidence-backed software modifications. Unlike conversational wrappers around local shells or hosted multi-tenant assistants, Vitna Code addresses the critical gap in coding agent trust: ensuring that every model choice has a recorded policy decision, every permission is bound to an exact normalized action digest, every effect has verifiable inputs and outputs, and every completion claim points to testable evidence.

Existing open coding agents (such as OpenCode, Codex CLI, and Hermes Agent) offer valuable architectural references. However:
- Wrapping an existing foreign daemon splits authoritative state across incompatible session, permission, extension, and persistence models.
- Foreign runtimes implement their own internal state machines, ambient authority models, or cloud-coupled synchronization pathways that violate Vitna's foundational honesty contract.
- Post-hoc retrofitting of exact-action capability tokens and append-only cryptographic event logging onto a foreign runtime would be more fragile and expensive than building a deliberate core from the first commit.

## Decision

1. **Vitna-Authoritative Rust Core**: Build a dedicated Rust core (`vitna-coded`, `vitna-runner`, and `vitna-git-broker`) as the sole authoritative owner of sessions, events, provider streaming, context assembly, capability policy, agent workspaces, tool brokering, and recovery.
2. **No Forking of Foreign Runtimes**: Do not fork or embed OpenCode, Codex, or Hermes as the product runtime. Leaf libraries with narrow, audited interfaces may be consumed if they pass our 6-point dependency criteria (ADR-0002, Dependency Matrix).
3. **Encoding the Vitna Honesty Contract**:
   - **Local First**: Full product capabilities operate with zero reliance on `vitna.ai` after installation and optional signed policy delivery.
   - **Customer-Held Credentials**: Credentials are resolved locally from the OS keychain, native workload identity, or customer credential helpers. Vitna servers never hold or transit provider credentials.
   - **No Code/Prompt Exfiltration**: Raw source code, prompts, diffs, terminal streams, and tool results are never transmitted to `vitna.ai`.
   - **Security Outside the Model**: The model proposes actions; deterministic policy and OS-enforced sandboxes decide and execute.
   - **Evidence Over Confidence**: A model assertion is never proof. All evidence carries explicit typed grades (`model_reported`, `broker_observed`, `sandbox_captured`, `independently_reproduced`, `remote_or_hardware_attested`).
   - **No Silent Effects**: Every Vitna-mediated effect entry point generates an explicit policy decision and append-only ledger event.
   - **No Silent Model Switch**: Fallback across model providers or SKUs can occur only at safe turn boundaries and must be visible in receipts.
   - **No Silent Retry of Ambiguous Mutations**: Interrupted commands or external mutations enter `needs_reconciliation` and are never automatically retried.
   - **No Shared Mutable Workspace**: Write-capable agents receive their own independent, disposable Git clone with isolated metadata.
   - **Repository Content Is Untrusted Input**: Instructions in repositories (`AGENTS.md`, `CLAUDE.md`, skills) can shape reasoning but can never widen security capabilities.
   - **Unknown Stays Unknown**: Unmeasured costs, missing test suites, and unverified outputs are labeled plainly as unknown.

## Consequences

### Positive
- Unified, uncompromised capability-security model spanning built-in tools, shell commands, MCP servers, skills, and child agents.
- Complete crash durability and recovery without state desynchronization between a wrapper and an underlying foreign server.
- Verifiable, tamper-evident receipts signed by local device identities with provable event hash chains.
- Reliable, first-class native support across Windows 11 (x64 and ARM64), macOS, and Linux.

### Negative / Trade-offs
- Slower initial time to first end-to-end demo compared to wrapping an existing Node or Python runtime.
- Requires building and maintaining specialized provider adapters, streaming codecs, and terminal UI layouts in Rust.
