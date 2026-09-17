# Phase 3 Gate Report: Trustworthy Solo-Agent Beta

- Status: PASSED
- Date: 2026-09-16
- Working product: Vitna Code (vitna)
- Phase: 3 (Trustworthy Solo-Agent Beta: Strong Sandbox, MCP, Keychain Secrets, Browser Verification)
- Repository: convexityos/vitna-code

## 1. Executive Summary

Phase 3 delivers the hardened trust boundaries of Vitna Code: strong OS sandbox runtime execution across Linux (Bubblewrap), macOS (Seatbelt), and Windows (AppContainer/Job Object); Model Context Protocol (MCP) client and capability brokering; customer-held local OS keychain secret storage; and browser verification evidence capture.

Untrusted external tools and MCP servers are brought strictly within Vitna's exact-action authorization model. Every MCP tool invocation generates a deterministic action digest and requires operator approval before execution.

## 2. Deliverables Audit

### 2.1 Strong OS Sandbox Runtime Enforcement (`crates/vitna-sandbox`)
1. `SandboxExecutor` (`crates/vitna-sandbox/src/executor.rs`):
   - Linux: Bubblewrap execution with `--unshare-net`, `--unshare-pid`, `--unshare-ipc`, read-only system binds, and scoped writable workspace mounts.
   - macOS: Seatbelt execution (`/usr/bin/sandbox-exec -p <profile>`) enforcing granular path and network denials.
   - Windows: AppContainer profile identity mapping, environment variable sanitization, and bounded execution timeouts.
   - Generates SHA-256 statement digests for all executed actions.
2. Integration Test (`crates/vitna-sandbox/tests/sandbox_execution_test.rs`):
   - Proves basic command execution, ambient environment secret scrubbing, and hard timeout termination.

### 2.2 Model Context Protocol (MCP) Client & Capability Broker (`crates/mcp`)
1. JSON-RPC 2.0 Wire Types (`crates/mcp/src/protocol.rs`):
   - Standard requests, responses, notifications, error codes, and tool descriptors (`initialize`, `tools/list`, `tools/call`).
2. `McpClient` (`crates/mcp/src/client.rs`):
   - Connects to MCP servers, handles capability negotiation, lists tools, and dispatches calls.
3. `McpToolBridge` (`crates/mcp/src/broker.rs`):
   - Adapts external MCP tools into the Vitna `Tool` trait.
   - Enforces exact-action digests over MCP arguments: `SHA256(mcp:{server}:{tool}:{args})`.
   - Strictly requires operator approval for all external MCP tool calls (`requires_approval = true`).
4. Integration Test (`crates/mcp/tests/mcp_client_test.rs`):
   - Tests handshake, tool discovery, capability bridging, and brokered tool execution.

### 2.3 Local OS Keychain Secret Storage (`crates/providers/src/keyring.rs`)
1. `KeyringStore`:
   - Unified interface for customer-held secrets (`get_secret`, `set_secret`, `delete_secret`).
   - Secure storage with environment variable fallback (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`).
   - Guarantees zero plaintext secret leakage to disk or third-party networks.

### 2.4 Browser Verification Evidence Capture (`crates/tools/src/browser_verify.rs`)
1. `BrowserVerifyTool`:
   - Inspects web application output, localhost dev servers, or local HTML documents.
   - Computes SHA-256 DOM digest and verifies expected content assertions.
   - Produces evidence outputs for inclusion in `VitnaRunReceiptV1` under grade `sandbox_captured`.
2. Standard tool registry expanded to 8 brokered tools (`crates/tools/src/lib.rs`).

## 3. Phase 3 Exit Certification

With Phase 3 complete and verified:
- External MCP servers cannot execute actions without explicit exact-action capability approval.
- Secrets are stored securely without disk plaintext exposure.
- OS sandboxes enforce process, network, and environment boundaries.
- The project is certified to exit Phase 3 and proceed to Phase 4 (Durable Multi-Agent Beta: DAG Scheduler, Per-Agent Clones, and Merge Queue).
