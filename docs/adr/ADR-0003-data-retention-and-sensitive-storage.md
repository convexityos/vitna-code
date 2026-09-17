# ADR-0003: Data Retention and Sensitive Payload Storage

- Status: Accepted
- Date: 2026-09-16
- Deciders: Vitna Core Engineering

## Context

Coding agents accumulate vast quantities of sensitive information: proprietary source code, user prompts, git diffs, environment variables, compiler errors, and external tool responses. If these are written as plaintext to local SQLite databases, SQLite Write-Ahead Logs (WAL), or content-addressed artifact caches:
- Backups, crash dumps, or disk indexing utilities can expose sensitive customer code.
- Malicious unprivileged processes or unauthorized host users might access sensitive historical runs.
- Ephemeral CI runners risk leaving unencrypted state remnants on shared runner disks.

At the same time, owner-only filesystem permissions (such as POSIX 0600 or Windows DACLs) provide access control against other users, but do not provide encryption at rest.

## Decision

1. **Envelope Encryption at Rest**:
   - Sensitive message payloads, tool arguments, raw outputs, and stored artifacts are encrypted before reaching SQLite, the SQLite WAL, or the filesystem.
   - A random, high-entropy per-user Data Encryption Key (DEK) is generated upon first daemon initialization.
   - The DEK is wrapped using a Key Encryption Key (KEK) obtained from the platform credential store (Windows DPAPI / Credential Manager, macOS Keychain, Linux Secret Service / Keyutils).
   - AES-256-GCM or ChaCha20-Poly1305 with authenticated tags is used for all encrypted payloads.

2. **Sealed and Unsealed States**:
   - If secure OS key storage is available, the daemon boots into the `sealed` state, unseals the DEK in memory, and operates with transparent encryption.
   - If protected key storage is unavailable (such as headless servers without a keyring or configured helper), the UI displays state as `unsealed-warning` and requires an explicit operator flag (`--allow-unsealed-storage`) before persisting sensitive records.

3. **Ephemeral Headless Mode**:
   - In CI or headless automation mode (`vitna run --ephemeral`), the DEK is held purely in memory and never written to disk.
   - Upon job completion or abnormal termination, all temporary workspace files, cached artifacts, and SQLite database files are cryptographically wiped and deleted.

4. **Retention Policies and Secure Deletion**:
   - Artifacts carry retention classes (`transient`, `session`, `permanent`).
   - Deletion purges database rows, indexes, encrypted artifacts, and workspaces, writing only a non-sensitive deletion tombstone if required by organization audit policy.
   - Exported receipts list omitted or redacted artifacts explicitly by digest and never imply they remain recoverable.

## Consequences

### Positive
- Zero plaintext proprietary code or sensitive tool payloads in SQLite databases, WAL files, or cache directories.
- Strong defense against offline disk theft and automated filesystem indexing.
- Predictable and configurable lifecycle management for large build artifacts and command logs.

### Negative / Trade-offs
- Slight CPU overhead for cryptographic encryption and decryption during streaming artifact writes (benchmarked at less than 1ms per typical tool output).
- In headless environments, operators must configure an explicit credential helper or opt into ephemeral mode.
