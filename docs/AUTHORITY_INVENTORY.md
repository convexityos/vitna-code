# Vitna Code Complete Authority Inventory

- Document Version: 1.0.0
- Date: 2026-09-16
- Status: Phase 0A Specification
- Authority: ADR-0001, ADR-0002, Threat Model

This inventory specifies every effect and access entry point mediated by Vitna Code. No capability exists outside this register. Every entry details:
1. **Subject**: The requesting entity (e.g., daemon turn, child agent, tool broker).
2. **Action**: The normalized operation name.
3. **Resource**: The target path, URL, process, or object.
4. **Deciding Authority**: The governing policy level deciding allow, ask, or deny.
5. **Enforcement Mechanism**: The OS or runtime boundary executing the check.
6. **Receipt Statement**: The cryptographic event record emitted into the run receipt.
7. **Failure/Denial Behavior**: The exact outcome upon denial, error, or ambiguity.

---

## 1. Repository Reads and Ranged Reads

- **Subject**: Inspect or Build agent turn; Tool broker.
- **Action**: `repo:read_file`, `repo:read_range`, `repo:list_directory`, `repo:search_ripgrep`.
- **Resource**: Files and directories inside the active workspace clone.
- **Deciding Authority**: Execution policy (`execution-policy.toml`), workspace bounds rules.
- **Enforcement Mechanism**: `vitna-runner` canonicalizes the path against the agent workspace root. Traversal attempts (`../`, symlinks to external host paths) are rejected by path normalization before OS file handle open.
- **Receipt Statement**: Emits `context_receipt` and `tool_call` event recording relative path, byte offset, length, and content SHA-256 hash.
- **Failure/Denial Behavior**: Returns structured `AccessDenied` or `NotFound` error to model. Never falls back to unconstrained host filesystem.

---

## 2. Repository Writes and Atomic Patch Application

- **Subject**: Build agent turn; Tool broker.
- **Action**: `repo:write_file`, `repo:apply_patch`.
- **Resource**: Target file path within the agent workspace clone.
- **Deciding Authority**: Execution policy; User approval (if outside pre-approved policy rule).
- **Enforcement Mechanism**: `vitna-runner` verifies the expected preimage SHA-256 hash. Performs atomic write via temporary file followed by atomic rename within the same filesystem mount. Enforces preserved line endings and UTF-8 validation.
- **Receipt Statement**: Emits `tool_call` event with action digest, relative path, preimage SHA-256 hash, and postimage SHA-256 hash.
- **Failure/Denial Behavior**: If preimage hash does not match current file content, the patch aborts with `PreimageMismatch`. The target file remains untouched.

---

## 3. Git Operations and Workspace Materialization

- **Subject**: Daemon orchestrator; Integration agent.
- **Action**: `git:create_mirror`, `git:create_agent_clone`, `git:snapshot_dirty`, `git:apply_changeset`.
- **Resource**: Vitna private mirror directory, disposable agent clone directories, primary repository checkout.
- **Deciding Authority**: Execution policy; Explicit user approval for final changeset apply to primary checkout.
- **Enforcement Mechanism**: Every host-side git command is built by `vitna_git_workspaces::host_git::command`, which points `core.hooksPath` at an empty directory this process owns, sets `core.fsmonitor=false`, clears each filter driver declared in `local` or `worktree` scope, and refuses to run at all when a protection cannot be established. `vitna-git-broker` does not itself invoke git. Disposable workspaces are file copies that exclude `.git`, so they carry no git metadata rather than self-contained metadata.
- **Receipt Statement**: Emits `workspace_snapshot` and `changeset_applied` events recording base commit SHA, dirty snapshot diff digest, and final merge commit SHA.
- **Failure/Denial Behavior**: If Git operations encounter conflicts or dirty tree collision, the action enters `needs_reconciliation` and stops. The primary checkout is never modified without explicit user approval.

---

## 4. Process Execution and PTY Sessions

- **Subject**: Build agent turn; Tool broker.
- **Action**: `process:exec_structured`, `process:pty_session`.
- **Resource**: Executable binary, arguments, environment, working directory.
- **Deciding Authority**: Execution policy rules; User approval binding exact action digest.
- **Enforcement Mechanism**: `vitna-runner` asks `vitna-sandbox` for an invocation that delivers the requested guarantee and refuses to run when it cannot get one. Bubblewrap on Linux and Seatbelt on macOS; **Windows has no backend**, so commands there are refused unless unsandboxed execution is explicitly approved, separately from any general auto-approve. The environment is cleared and rebuilt. The applied backend and enforcement level are recorded in the action journal and in the receipt's runner statement, and only code that applied a backend sets `VITNA_SANDBOX`.
- **Not implemented**: binary resolution without ambient `PATH`, binary content hashing, CPU/memory/process limits, and PTY attachment. `SandboxConfig` carries limit fields that nothing applies.
- **Receipt Statement**: Emits `runner_execution_statement` signed by local runner identity, recording action digest, canonical cwd, exit code, stdout hash, and stderr hash.
- **Failure/Denial Behavior**: If unapproved, denied by policy, or sandbox initialization fails, execution is halted immediately. In the event of a timeout or abort, the full process tree is terminated. Interrupted commands enter `needs_reconciliation` and are never retried automatically.

---

## 5. Network Access and Egress Destinations

- **Subject**: Tool broker (web search, web fetch); Model provider transport.
- **Action**: `net:http_fetch`, `net:dns_resolve`, `net:provider_egress`.
- **Resource**: Remote IP address, port, and fully qualified domain name (FQDN).
- **Deciding Authority**: Organization execution policy; Egress destination allowlist.
- **Enforcement Mechanism**: Network namespace isolation (Linux), firewall filtering (Windows/macOS), and daemon egress HTTP client proxy enforcement. Subprocesses in `vitna-runner` have network interfaces disabled (`--unshare-net` on Linux, network isolation token on Windows) unless explicit network authority is granted.
- **Receipt Statement**: Emits `egress_call` event with target domain, request method, payload hash, response status code, and response payload hash.
- **Failure/Denial Behavior**: Connection attempt blocked at socket layer. Returns immediate `NetworkDestinationDenied` error to caller.

---

## 6. Credentials and Secret Resolution

- **Subject**: Provider adapters; Secure tool instances.
- **Action**: `secret:resolve_key`, `secret:inject_token`.
- **Resource**: Named secret handle (e.g., `openai-api-key`, `github-app-token`).
- **Deciding Authority**: Organization policy; Local credential provider configuration.
- **Enforcement Mechanism**: Daemon queries the configured credential provider (OS Keychain, customer credential helper, or non-persisted memory store). **Ambient environment variable scanning is strictly disabled**.
- **Receipt Statement**: Emits secret access record naming the **secret handle name only**. Secret values are never logged, hashed into public receipts, or leaked to subprocesses.
- **Failure/Denial Behavior**: If credential cannot be resolved or permission is denied, the billable or authenticated request fails closed with `AuthenticationRequired`.

---

## 7. Model Provider Transport and Routing

- **Subject**: Daemon provider orchestrator.
- **Action**: `provider:send_request`, `provider:stream_response`.
- **Resource**: Upstream provider endpoint (OpenAI, Anthropic, Vitna Relay, Vitna Anchor, local endpoint).
- **Deciding Authority**: Signed model policy (`model-policy.json`); User configuration.
- **Enforcement Mechanism**: Daemon provider adapter verifies model SKU capability, signs request envelope, enforces rate limits and context budgets, and validates provider TLS certificate.
- **Receipt Statement**: Emits `model_call` event recording provider name, model SKU, request digest, usage token counts (prompt, completion, cached), and known financial cost.
- **Failure/Denial Behavior**: Provider errors or stream interruptions abort the turn. Fallback to a secondary model occurs only if allowed by signed policy and only at clean turn boundaries before visible output or side effects.

---

## 8. Model Context Protocol (MCP) Principals

- **Subject**: MCP host manager; Extension subagent.
- **Action**: `mcp:spawn_server`, `mcp:call_tool`, `mcp:read_resource`.
- **Resource**: Local MCP stdio command or remote Streamable HTTP endpoint.
- **Deciding Authority**: Execution policy; MCP server permission manifest; User approval.
- **Enforcement Mechanism**: Each MCP server is treated as an untrusted external principal. Local stdio servers run inside isolated runner sandboxes with bounded memory, process limits, and restricted filesystem mounts.
- **Receipt Statement**: Emits `mcp_tool_call` event with server ID, tool definition digest, argument digest, approval ID, and result digest.
- **Failure/Denial Behavior**: Malformed responses, timeouts, or policy violations result in tool call rejection and server isolation without crashing the main daemon.

---

## 9. System Updates and Package Installation

- **Subject**: CLI updater; Package manager.
- **Action**: `system:check_update`, `system:apply_binary_update`.
- **Resource**: Signed Vitna release artifacts and update manifests.
- **Deciding Authority**: User explicit instruction; Release signature verification key.
- **Enforcement Mechanism**: Cryptographic Ed25519 signature validation against pinned root keys; SHA-256 release artifact hash verification; atomic binary replacement.
- **Receipt Statement**: Emits system telemetry event recording previous binary SHA-256, target release version, and signature verification result.
- **Failure/Denial Behavior**: Any signature mismatch, hash failure, or untrusted signer terminates the update immediately with zero modifications.

---

## 10. Model Policy and Price Tape Ingestion

- **Subject**: Local daemon policy manager.
- **Action**: `policy:ingest_model_policy`, `policy:ingest_price_tape`.
- **Resource**: `model-policy.json`, signed price tape documents.
- **Deciding Authority**: Pinned Vitna public signing key (`VITNA_PUBLIC_KEY`).
- **Enforcement Mechanism**: Verifies cryptographic Ed25519 signature over canonical JSON document bytes; verifies expiry timestamp.
- **Receipt Statement**: Emits `policy_loaded` event with policy document digest, signer key ID, and valid date range.
- **Failure/Denial Behavior**: An unsigned, modified, or expired policy document is rejected. The daemon falls back to previously confirmed local static defaults and flags unknown prices as unknown.
