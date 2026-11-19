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
- **Action**: `repo:read_file`, `repo:read_range`, `repo:list_directory`, `repo:search_ripgrep`, `repo:verify_document`.
- **Resource**: Files and directories inside the active workspace clone.
- **Deciding Authority**: Execution policy (`execution-policy.toml`), workspace bounds rules.
- **Enforcement Mechanism**: `vitna-tools` (`workspace_fs.rs`) rejects lexical traversal (`../`, absolute paths outside the root) in `path_safety.rs`, then canonicalizes the requested path and requires the real location, every link resolved, to lie inside the canonicalized workspace root. Only regular files are opened (never a FIFO, socket, device or directory), without following a final link and without blocking. The opened handle is then asked where it lives (`/proc/self/fd` on Linux, `F_GETPATH` on macOS, `GetFinalPathNameByHandleW` on Windows) and must still be inside the root, so a link swapped in between the check and the open is refused. `read_file` loads at most 16 MiB and returns at most 256 KiB per call, with an explicit truncation notice; `search_code` never follows links and reports unreadable paths as a partial result rather than as no matches. `browser_verify` is a read under this section and not a network action: it digests a workspace document through the same containment, refuses a remote target rather than fetching it (section 5), and refuses a document larger than 16 MiB rather than digesting a prefix of it. Its report states that the digest is of the bytes on disk and that no renderer ran, so a receipt cannot read it as a captured DOM.
- **Receipt Statement**: Emits `context_receipt` and `tool_call` event recording relative path, byte offset, length, and content SHA-256 hash.
- **Failure/Denial Behavior**: Returns structured `AccessDenied` or `NotFound` error to model. Never falls back to unconstrained host filesystem.

---

## 2. Repository Writes and Atomic Patch Application

- **Subject**: Build agent turn; Tool broker.
- **Action**: `repo:write_file`, `repo:apply_patch`.
- **Resource**: Target file path within the agent workspace clone.
- **Deciding Authority**: Execution policy; User approval (if outside pre-approved policy rule).
- **Enforcement Mechanism**: `vitna-tools` (`workspace_fs.rs`) applies the containment of section 1 to the write target and every directory it creates, reads the preimage through the verified handle, and checks `expected_preimage_hash` before anything on disk changes. The write happens in place through that handle and is flushed with `sync_all`; the file is then read back and compared byte for byte, and the postimage hash is computed from the bytes read back. A file that does not hold exactly the requested bytes (short write, deferred write error, anything rewriting it) fails the tool call, so no postimage is recorded for it. Bytes are written exactly as given, line endings included. The write is not atomic: a crash mid-write can leave a partial file. Files over 16 MiB are not replaced.
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
- **Merge into the primary checkout**: `vitna-git-broker` (`tree.rs`) resolves each changeset path one component at a time from the canonicalized root, in the primary checkout and in the agent workspace alike, and follows no link on either side, including a link that stays inside the tree: a changeset describes regular files only, so it cannot say whether to write through such a link or replace it. Every directory on the way must be a real directory, and the path a regular file or nothing. Files are opened with O_NOFOLLOW | O_NONBLOCK on Unix, and the opened handle must report exactly the walked path (`/proc/self/fd`, `F_GETPATH`, `GetFinalPathNameByHandleW`). Missing directories are created one at a time, never through a link. Each preimage is checked again through the handle about to be written, and every source must still hash to the postimage its change attests before the first write. Permission bits travel with the bytes. `apply_changeset` and `MergeQueue` share this one write path.
- **Failure/Denial Behavior**: If Git operations encounter conflicts or dirty tree collision, the action enters `needs_reconciliation` and stops. The primary checkout is never modified without explicit user approval. A changed path that the primary checkout holds as a link (at the file or at any directory above it), or as a directory, FIFO, socket or device, fails `inspect_changes`, and at merge time it is a `MergeConflict` found before anything is written, as is a file standing where a directory belongs. A changeset path that is absolute or climbs out with `..`, or a source that no longer matches its postimage, is an error, and nothing is written. The merge is not atomic: a refusal while writing (the checkout changed after validation) or an I/O error stops it with an error naming the files already written.

---

## 4. Process Execution and PTY Sessions

- **Subject**: Build agent turn; Tool broker.
- **Action**: `process:exec_structured`, `process:pty_session`.
- **Resource**: Executable binary, arguments, environment, working directory.
- **Deciding Authority**: Execution policy rules; User approval binding exact action digest.
- **Enforcement Mechanism**: `vitna-runner` asks `vitna-sandbox` for an invocation that delivers the requested guarantee and refuses to run when it cannot get one. Guarded OS confinement is implemented and enforced in CI across all three desktop platforms: Bubblewrap on Linux (`--unshare-net`, read-only host mounts, writable workspace, read-only `.git`/`.vitna`), Seatbelt on macOS, and native Win32 AppContainer on Windows x64 and ARM64 (per-workspace profile SID, protected read-only `.git/*`/`.vitna` DACLs, zero network capability by default, and `KILL_ON_JOB_CLOSE` Job Object process-tree teardown). Commands are refused whenever no backend is available unless unsandboxed execution (`--allow-unsandboxed`) is explicitly approved, separately from general auto-approve. The environment is cleared and rebuilt. The applied backend and enforcement level are recorded in the action journal and in the receipt's runner statement, and only code that applied a backend sets `VITNA_SANDBOX`. Interactive PTY sessions (`AttachTerminal`) run through `portable-pty` with THR-07 output sanitization.
- **Not implemented**: `strong` isolation (container/VM), binary resolution without ambient `PATH`, binary content hashing, and CPU/memory/process limits (`SandboxConfig` carries limit fields that nothing applies yet).
- **Receipt Statement**: Emits `runner_execution_statement` signed by local runner identity, recording action digest, canonical cwd, exit code, stdout hash, and stderr hash.
- **Failure/Denial Behavior**: If unapproved, denied by policy, or sandbox initialization fails, execution is halted immediately. In the event of a timeout or abort, the full process tree is terminated. Interrupted commands enter `needs_reconciliation` and are never retried automatically.

---

## 5. Network Access and Egress Destinations

- **Subject**: Tool broker (`http_fetch`, remote `browser_verify`); Model provider transport.
- **Action**: `net:http_fetch`, `net:dns_resolve`, `net:provider_egress`.
- **Resource**: Remote IP address, port, and fully qualified domain name (FQDN).
- **Deciding Authority**: Organization execution policy; Operator `--allow-egress <host>` destination allowlist; Exact-action user approval (`effect_class == "network"`).
- **Enforcement Mechanism**: Subprocesses in `vitna-runner` have network interfaces disabled (`--unshare-net` on Linux, zero network capability in Windows AppContainer) unless explicit network authority is granted. Tool-initiated HTTP/HTTPS requests (`http_fetch` and remote targets passed to `browser_verify`) execute exclusively through `vitna_tools::EgressBroker` (`crates/tools/src/egress.rs`), which fails closed with `NetworkDestinationDenied` when no allowlist is configured or when a host does not match an operator-specified bare host or subdomain wildcard (`*.domain.tld`). Blanket wildcards (`*`), URLs with userinfo, and host:port entries are refused. Before connecting, `EgressBroker` resolves DNS, rejects private/link-local/cloud-metadata IP ranges (unless a loopback/literal IP is explicitly allowlisted), pins the validated socket address (`ClientBuilder::resolve`, `.no_proxy()`, `Policy::none()`), re-validates every redirect hop (maximum 5 hops), and streams response bodies up to a strict 4 MiB cap (refusing oversized bodies rather than hashing a prefix). `OrchestrationEngine` requires exact-action approval with `effect_class == "network"` for `http_fetch` and remote `browser_verify` calls while keeping local workspace `browser_verify` calls read-only (`effect_class == "read"`).
- **Not implemented**: No headless JavaScript browser renderer runs in `browser_verify` (when a remote URL is allowlisted and approved, `browser_verify` fetches raw HTTP bytes through `EgressBroker` and states explicitly that no JS renderer ran and the digest is not a rendered DOM). Dedicated per-process OS firewall proxy filtering beyond Linux `--unshare-net` and Windows AppContainer zero-network tokens is not yet implemented. Model provider transport is governed separately by section 7 and recorded via `model_call` and local `TelemetryCollector` egress accounting.
- **Receipt Statement**: Emits a hash-chained `vitna.audit.v1.EgressCall` event, a `broker_observed` evidence item, and an `EgressCallRecord` inside `VitnaRunReceiptV1.egress_calls` recording `target_domain`, `method`, `redacted_url` (query string and fragment stripped), `request_sha256`, `status_code`, `response_sha256`, `bytes_sent`, `bytes_received`, and `latency_ms`, validated offline by `ReceiptVerifier`.
- **Failure/Denial Behavior**: Unapproved calls never invoke the broker. Unallowlisted hosts, forbidden IP ranges, disallowed redirect targets, and oversized payloads fail closed with `NetworkDestinationDenied` (or size-limit refusal) without emitting any fabricated digest or passing assertion.

---

## 6. Credentials and Secret Resolution

- **Subject**: Provider adapters; Secure tool instances.
- **Action**: `secret:resolve_key`, `secret:inject_token`.
- **Resource**: Named secret handle (e.g., `openai-api-key`, `github-app-token`).
- **Deciding Authority**: Organization policy; Local credential provider configuration.
- **Enforcement Mechanism**: `vitna-providers` resolves a provider API key only through a `CredentialSource` handed to `CredentialResolver` explicitly, and from nowhere else. The one production source is `KeyringStore`, the operating system's credential store: Windows Credential Manager, the macOS login keychain, or the Secret Service on Linux, with one entry per provider under the service `vitna-code`. **Ambient environment variable scanning is disabled**: nothing in the crate reads an environment variable. `crates/providers/tests/no_ambient_credentials.rs` sets `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` and both `*_BASE_URL` variables and checks that no resolver and no store returns any of them, and `providers_read_no_environment.rs` fails if any source file in the crate reads the environment at all. The endpoint a request goes to is not read from the environment either, because whatever sets it decides where the key is sent.
- **Not implemented**: no customer credential helper and no workload identity source exists; each would be a further `CredentialSource` implementation. The non-persisted memory store (`MemoryCredentialStore`) is compiled only for tests. No code outside `vitna-providers` constructs a `CredentialResolver` yet, so no turn resolves a key through this path, and `secret:inject_token` has no implementation.
- **Receipt Statement**: Specified as a secret access record naming the **secret handle name only**. Not implemented: no such record is emitted. What `vitna-providers` guarantees today is narrower: `ApiKey` and `ProviderCredentials` never print a key through `Debug`, and no `CredentialError` carries one.
- **Failure/Denial Behavior**: A key missing from the store fails closed with `CredentialError::AuthenticationRequired`, naming the provider and the store. A store that cannot be reached, is locked, or refuses access fails with `CredentialError::StoreFailed`, and nothing else is tried.

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
