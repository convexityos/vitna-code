# ADR-0005: IPC Transport and Wire Framing

- Status: Accepted
- Date: 2026-09-16
- Deciders: Vitna Core Engineering

## Context

The Vitna Agent Protocol connects clients (the interactive TUI, headless CLI, machine-readable JSONL bridges, and future desktop apps) to the local `vitna-coded` daemon. The transport must:
- Prevent unauthorized local processes from injecting commands, listening to private code streams, or modifying security approvals.
- Provide sub-millisecond local message delivery and streaming responsiveness under heavy model output.
- Support robust schema evolution, forward compatibility, and client reconnects without event loss.
- Work natively across Windows, macOS, and Linux without external network ports.

## Decision

1. **Owner-Only Local Transport**:
   - **Unix (macOS & Linux)**: Unix Domain Sockets (`UDS`) placed in `$XDG_RUNTIME_DIR/vitna/vitna.sock` or `~/.vitna/vitna.sock` with POSIX permissions `0600` (owner read/write only).
   - **Windows**: Windows Named Pipes (`\\.\pipe\vitna-<user_sid>`) with explicit discretionary access control lists (DACL) granting full control solely to the current user SID and Local System, denying network access and other local users.
   - **No Default TCP Binding**: The daemon never binds to loopback TCP (`127.0.0.1`) by default, eliminating attacks from local browsers via WebSockets, DNS rebinding, or Cross-Origin Resource Sharing (CORS) exploits.

2. **Framing and Protocol Buffers**:
   - Every message travels in a length-prefixed binary frame: a 4-byte big-endian frame length prefix followed by a serialized Protobuf `ProtocolEnvelope`.
   - The envelope contains:
     - Protocol major and minor version numbers.
     - Schema version.
     - Stable type URL string (e.g., `type.vitna.ai/vitna.protocol.v1.SubmitTurn`).
     - Session ID, Run ID, monotonic sequence number, and idempotency key.
     - Opaque payload bytes.

3. **Strict Admission Order**:
   To ensure security and defense against malformed frames:
   - Step 1: Enforce strict frame size limit (maximum 16MB per frame; frames exceeding this are dropped with connection termination).
   - Step 2: Validate local IPC peer identity using OS credentials (`SO_PEERCRED` on Linux, `LOCAL_PEERCRED` on macOS, `GetNamedPipeHandleState` on Windows).
   - Step 3: Verify protocol version compatibility and session authorization.
   - Step 4: Decode and schema-validate the command payload.
   - Step 5: Classify sensitive fields and encrypt before durable persistence.
   - Step 6: Commit command to SQLite WAL and acknowledge the client.

4. **Event Streaming and Reconnection**:
   - Every event emitted by the daemon has a strictly monotonic `sequence` number.
   - Reconnecting clients supply `resume_after_sequence`. The daemon streams all missing events since that sequence, or issues a compacted state snapshot if historical deltas have been pruned.

## Consequences

### Positive
- Immune to browser-based local port scanning and cross-site websocket attacks.
- High-efficiency binary serialization with predictable memory allocations.
- Reconnections survive temporary client crashes or terminal window closures without dropping events.

### Negative / Trade-offs
- Binary framing requires generated code or Protobuf tooling for external client implementations.
- Windows named pipe security descriptors require platform-specific Win32 API calls (`SetSecurityInfo`, `InitializeSecurityDescriptor`).
