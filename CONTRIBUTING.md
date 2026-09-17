# Contributing to Vitna Code

Thank you for contributing to Vitna Code.

Vitna Code enforces strict security, honesty, and code hygiene standards to maintain verifiable execution guarantees across every supported platform.

## Architectural Rules

1. **Rust Core Authority**: All session state, context assembly, provider routing, approvals, and mutations must be owned by `vitna-coded`. Do not introduce secondary or external runtimes to manage authoritative state.
2. **Hardened Boundary**: Untrusted repository processes must never run inside the daemon process. All process execution and filesystem modifications must transit `vitna-runner` or `vitna-git-broker`.
3. **Exact Action Digest**: Approvals must bind to a deterministic hash of the action definition, argument digest, target executable digest, cwd, mounts, environment names, and limits.
4. **No Ambient Credentials**: Never scan the environment for API keys. Credentials must be resolved explicitly through the credential provider interface or OS keychain.
5. **No Silent Retries**: An interrupted or ambiguous mutating effect enters `needs_reconciliation` state. It is never automatically retried.
6. **No Em-Dashes**: Per project rules, em-dashes are banned across the repository, including source code comments and markdown documentation. Use hyphens, colons, or parentheses.

## Pull Request Guidelines

Every pull request must be scoped to a single logical deliverable and include:

1. A clear acceptance statement describing the verified change.
2. Unit tests, integration tests, and fault-injection tests where effect semantics are altered.
3. Updated documentation and Architectural Decision Records (ADRs) when interfaces or boundaries change.
4. Schema migration notes when persistent stores or wire protocols change.
5. An update to `IMPLEMENTATION_STATUS.md` citing verifiable evidence.

## Branch and Verification Flow

- Preflight check: Ensure all workspace crates compile cleanly with no warnings under `cargo clippy`.
- Formatting: All Rust code must be formatted via `cargo fmt --all`.
- Cross-platform check: Changes must build cleanly across Windows 11 (x64 and ARM64), macOS, and Linux.
