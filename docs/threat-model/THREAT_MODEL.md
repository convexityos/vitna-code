# Vitna Code Threat Model

- Document Version: 1.0.0
- Date: 2026-09-16
- Status: Phase 0A Specification
- Authority: ADR-0001, ADR-0002

## 1. System Overview and Security Goals

Vitna Code is a local-first, tamper-evident coding agent. Its primary security objective is to allow developers and automated workflows to leverage autonomous models on complex, dirty repositories without exposing host environments, credential material, or administrative version control structures to compromise or silent mutation.

The foundational security guarantee is:
> The model proposes. Deterministic policy and OS enforcement decide. Lower-trust content cannot expand authority, bypass sandboxing, or alter evidence.

## 2. Trust Class Hierarchy

Vitna Code establishes a strict, total ordering of trust classes (from highest authority to lowest authority):

```
Level 1: Product Safety Policy (immutable kernel limits, hard denials)
   v
Level 2: Organization Policy (enterprise security baselines, enforced budgets)
   v
Level 3: User Policy & Explicit Instructions (runtime CLI flags, user approvals)
   v
Level 4: Trusted Project Instructions (locally verified, explicitly trusted rules)
   v
Level 5: Repository Content (untrusted files, untrusted instructions, READMEs)
   v
Level 6: Remote Content & Tool Output (web fetch results, API responses, subprocess output)
```

### Invariants Governing Trust Classes:
1. Denials issued by a higher level cannot be overridden by a grant from a lower level.
2. Repository content (Level 5) and remote tool output (Level 6) are untrusted input. They may influence model reasoning but can never grant capabilities, change execution policy, read credentials, or self-verify evidence.
3. Child agents inherit at most the capabilities of their parent (attenuation only; no privilege escalation).

## 3. The Seven Trust Boundaries

```
[External World / Model Providers]
               |
        Boundary 1 (TLS, Dialect Parsing, Egress Control)
               v
     +-------------------+       Boundary 6 (Owner-only IPC, DACL)
     |    vitna-coded    | <====================================== [Client: TUI/CLI]
     |   (Orchestrator)  |
     +-------------------+
       |       |       |
       |       |       +---> Boundary 5 (MCP / Skills / Hooks: Capability Principals)
       |       |
       |       +-----------> Boundary 7 (Encrypted SQLite WAL, Keychain, KEK/DEK)
       |
  Boundary 4 (Mirror vs Clone, No Shared .git)
       |
       v
+------------------+
| vitna-git-broker |
+------------------+
       |
  Boundary 3 (Sandboxed Process Execution, Action Journal)
       v
+------------------+
|   vitna-runner   | ===> Boundary 2 (Untrusted Repo Code / Hostile Workspace)
+------------------+
```

### Boundary 1: External Model Providers and Network Egress
- **Separation**: The local daemon connects over HTTPS to configured model providers (OpenAI, Anthropic, Vitna Relay, local endpoints).
- **Threats**: Man-in-the-middle attacks, malicious prompt injections reflected from web endpoints, compromised provider infrastructure injecting poisoned tool calls, unauthorized data exfiltration over egress channels.
- **Controls**: Strict TLS validation using platform root certificates; zero provider token scraping from the ambient environment; deterministic context receipts recording exact sent bytes; local-only mode enforcing zero hosted egress.

### Boundary 2: Untrusted Repository Content and Workspace Files
- **Separation**: Repository files checked out from the internet are inherently untrusted.
- **Threats**: Prompt injection hidden in comments or documentation (`AGENTS.md`, `CLAUDE.md`), path traversal exploits (`../../etc/passwd`), symbolic link / junction traps pointing to sensitive host directories, malicious git configuration files.
- **Controls**: Repository instructions are classified as Level 5 (cannot expand capabilities); canonical path resolution prevents directory traversal; symlinks pointing outside the workspace boundary are strictly rejected.

### Boundary 3: Tool Execution and Subprocesses (Runner Sandbox)
- **Separation**: Subprocesses spawned to compile code, run tests, or execute linters run inside `vitna-runner` under OS sandbox isolation.
- **Threats**: Hostile build scripts (`build.rs`, `setup.py`, `package.json` scripts), fork bombs, resource starvation, unauthorized network access, host credential theft, escaping to the host filesystem.
- **Controls**: Linux Bubblewrap/Landlock/seccomp, Windows AppContainer/Restricted Tokens/Job Objects, macOS native sandbox profiles; minimal environment variables; non-secret environment digests; explicit read-only and workspace mounts; resource and time limits.

### Boundary 4: Git Metadata and Workspace Isolation
- **Separation**: The primary repository's checkout and its `.git` directory must remain pristine and unreachable.
- **Threats**: Malicious hooks (`.git/hooks/pre-commit`), git filter drivers, git aliases, malicious git alternates pointing to private repositories, dirty tree contamination.
- **Controls**: Host-side git runs only through `vitna_git_workspaces::host_git::command`, which points `core.hooksPath` at an empty directory, sets `core.fsmonitor=false`, and clears every repo-scoped filter driver. `vitna-git-broker` itself does not invoke git; it compares and merges file contents in memory.
- **Not yet true of this codebase**: there is no private mirror. `AgentWorkspaceManager` copies files and skips `.git` entirely, so an agent workspace has no git metadata of its own rather than self-contained metadata. The claim that the host `.git` is never reachable by a spawned process depends on the runner sandbox, which is tracked separately.

### Boundary 5: Extension Principals (MCP Servers, Skills, Hooks)
- **Separation**: External MCP tools, skill scripts, and out-of-process hooks run as distinct capability principals.
- **Threats**: Rogue MCP server attempting privilege escalation, prompt injection via tool descriptions, denial of service via stalled responses, uncontrolled network access.
- **Controls**: Per-server capability isolation; individual tool definition digests; bounded timeouts and output spooling; explicit user approval required for effectful operations; pinned MCP protocol versions.

### Boundary 6: Client-to-Daemon Local IPC
- **Separation**: Local communication between TUI/CLI clients and `vitna-coded`.
- **Threats**: Malicious unprivileged local processes connecting to the daemon to execute unauthorized commands or siphon code streams; cross-site WebSocket hijacking from browsers.
- **Controls**: Owner-only Unix domain sockets (mode `0600`) or Windows Named Pipes with strict user SID DACLs; zero loopback TCP listeners; monotonic sequence numbers and client handshake verification.

### Boundary 7: Local Persistent Storage, Credential Stores, and Key Material
- **Separation**: Data at rest on the local host storage.
- **Threats**: Offline disk theft; inspection of SQLite databases and Write-Ahead Logs; unauthorized extraction of cached API keys.
- **Controls**: All sensitive message bodies, tool arguments, diffs, and artifacts are encrypted before hitting disk (ADR-0003); per-user Data Encryption Key (DEK) wrapped via OS Keychain (KEK); zero plaintext in SQLite WAL; secure deletion of ephemeral sessions.

## 4. Attacker Profiles and Assumptions

| Attacker Profile | Capabilities | Motivations |
|---|---|---|
| **Hostile Repository Author** | Commits malicious instructions, crafted symlinks, deceptive test cases, or poisoned build scripts to a public repository. | Achieve remote code execution on the developer's machine; steal API credentials; exfiltrate local files. |
| **Malicious MCP Server / Extension** | Serves dynamic tool schemas, injects prompt instructions in responses, attempts network requests to unauthorized destinations. | Pivot from an authorized integration into host command execution; exfiltrate proprietary code. |
| **Compromised Model Provider** | Emits malformed JSON, invalid tool arguments, or adversarial tool sequences. | Trigger parsing bugs, cause unlogged side effects, or bypass exact-action approval bounds. |
| **Local Unprivileged Host Process** | Operates on the same host operating system under a separate user account, or a sandboxed browser process. | Connect to local daemon IPC, inject commands, or read SQLite database files. |

## 5. High-Risk Threat Vectors and Mandatory Test Matrix

Every threat below is mapped to an automated test plan in `evals/`:

| Threat ID | Threat Description | Enforcement Mechanism | Mapped Test Suite |
|---|---|---|---|
| **THR-01** | Prompt Injection via Repository Instruction | Hierarchy enforces Level 5 bounds; capabilities cannot widen. | `evals/security/test_prompt_injection.rs` |
| **THR-02** | Path Traversal / Symlink Junction Escape | `vitna-runner` canonicalizes paths, verifies bounds against workspace root. | `evals/security/test_path_traversal.rs` |
| **THR-03** | Time-of-Check to Time-of-Use (TOCTOU) Race | Preimage hash verification; atomic file replacements. | `evals/security/test_atomic_replace.rs` |
| **THR-04** | Git Hook / Filter Execution Escape | `vitna_git_workspaces::host_git::command` is the sole constructor for host-side git: empty `core.hooksPath`, `core.fsmonitor=false`, repo-scoped filter drivers cleared, no repository-selected pager or external diff. Aliases need no control because they cannot override a builtin. | `crates/git-workspaces/tests/host_git_hardening.rs`, `crates/git-workspaces/tests/no_bare_git_invocations.rs`, `crates/tools/tests/git_status_hardening.rs` |
| **THR-05** | Shell Injection via Concatenation | Direct structured `argv` execution preferred; strict shell escaping. | `evals/security/test_shell_sanitization.rs` |
| **THR-06** | Fork Bomb / Process Resource Exhaustion | OS Job Objects (Windows) and cgroups / PID limits (Linux). | `evals/security/test_resource_limits.rs` |
| **THR-07** | Terminal Escape Injection (OSC 52 Clipboard Hijack) | Terminal sanitizer strips dangerous ANSI, OSC, and bidi control codes. | `evals/security/test_terminal_sanitizer.rs` |
| **THR-08** | Ambient Credential Exfiltration | Adapters use explicit credential provider interface; no env scraping. | `evals/security/test_credential_leakage.rs` |
| **THR-09** | Daemon Crash During Command Execution | Durable 5-step action journal; recovery to `needs_reconciliation`. | `evals/fault_injection/test_crash_recovery.rs` |
| **THR-10** | Cross-Agent Workspace Pollution | Per-agent disposable clones; single integration agent ownership. | `evals/security/test_agent_workspace_isolation.rs` |
