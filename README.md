# Vitna Code

> Working product name: Vitna Code  
> CLI binary: `vitna`  
> Repository: `convexityos/vitna-code`  
> Working line: The coding terminal that leaves a receipt.

Vitna Code is a local-first, tamper-evident coding agent built with a durable local daemon, an ergonomic terminal client, a hardened execution runner, and an append-only evidence ledger.

Rather than acting as another conversational wrapper around an unconstrained shell, Vitna Code makes software modifications verifiable and accountable:

- Every model choice has a recorded policy decision.
- Every permission is bound to an exact, normalized action digest.
- Every effect entry point mediated by Vitna has explicit inputs, outputs, authority, and provenance.
- Every persistent source change in the receipted final change set has preimage and postimage hashes.
- Every completion claim points to tests, checks, artifacts, or an explicit unknown.
- Every important run can be exported as a portable, signed receipt verifiable offline without Vitna cloud services.

## The Vitna Honesty Contract

Vitna Code inherits and preserves the architectural contract of Vitna:

1. **Local first**: The product functions completely offline without contacting `vitna.ai` after initial installation and optional signed policy delivery.
2. **Customer-held credentials**: Provider credentials are resolved locally via OS keychain, provider-native workload identity, or customer credential helpers. Credentials never transit Vitna servers.
3. **No raw data exfiltration**: Raw source code, prompts, diffs, terminal streams, and tool results are never sent to `vitna.ai`.
4. **Security outside the model**: The model proposes actions; deterministic policy and OS-enforced sandboxes decide.
5. **No silent effects**: Every Vitna-mediated effect entry point generates a policy decision and an append-only ledger event.
6. **No silent model switch**: Fallbacks only occur at safe turn boundaries and are visible in receipts.
7. **No silent retry of ambiguous mutations**: Interrupted commands or external mutations enter `needs_reconciliation` state and are never retried automatically.
8. **Unknown stays unknown**: Incomplete test coverage, unpriced models, and uncertain external effects are plainly labeled.

## Architecture

Vitna Code is structured as a Vitna-authoritative Rust core:

```
Terminal UI / CLI        CI / JSONL Streams        Future Desktop
        \                        |                       /
         +------------- Vitna Agent Protocol ------------+
                                 |
                         local vitna-coded
                 orchestration, policy, context,
                 provider adapters, task scheduler
                     SQLite WAL event store
                   content-addressed artifacts
                       /                  \
              model providers          tool broker
           direct, Relay, Anchor      /     |     \
                                vitna-runner Git broker  MCP
                              isolated jobs mirror/apply
```

- **Clients**: Interactive TUI (`apps/vitna-tui`), non-interactive CLI (`apps/vitna-cli`), and machine-readable JSONL stream.
- **Local Daemon (`crates/daemon`)**: Owner-scoped local process listening on owner-only Unix domain socket or Windows named pipe. Owns sessions, context assembly, provider calls, approvals, and durable transitions.
- **Execution Runner (`crates/vitna-runner`)**: Hardened Rust process boundary executing untrusted commands and repository file edits under OS-level sandboxing (Bubblewrap/Landlock, AppContainer, or isolated containers).
- **Git Broker (`crates/git-broker`)**: Trusted broker managing private repository mirrors and materializing disposable, self-contained agent workspaces with isolated Git metadata.
- **Evidence and Receipts (`crates/receipts`)**: Generates append-only cryptographic event hash chains and `vitna-run-receipt-v1` documents.
- **Offline Verifier (`crates/vitna-receipt-verify`)**: Standalone, independent verifier checking receipt integrity, Merkle roots, and signatures without running the daemon.

## Project Status

Vitna Code is currently in **Phase 0A** (Contracts, Threat Modeling, Platform Guarantees, and Dependency Review).

See [IMPLEMENTATION_STATUS.md](file:///c:/Users/baseb/convexity/vitna-code/IMPLEMENTATION_STATUS.md) and [PHASE_0A_GATE_REPORT.md](file:///c:/Users/baseb/convexity/vitna-code/docs/PHASE_0A_GATE_REPORT.md) for detailed evidence and verification logs.

## Documentation Index

- [Architecture and Product Invariants (ADR-0001)](file:///c:/Users/baseb/convexity/vitna-code/docs/adr/ADR-0001-architecture-and-product-invariants.md)
- [Trusted Core and Execution Boundary (ADR-0002)](file:///c:/Users/baseb/convexity/vitna-code/docs/adr/ADR-0002-trusted-core-and-execution-boundary.md)
- [Data Retention and Sensitive Payload Storage (ADR-0003)](file:///c:/Users/baseb/convexity/vitna-code/docs/adr/ADR-0003-data-retention-and-sensitive-storage.md)
- [Persistence Engine and Storage Actor (ADR-0004)](file:///c:/Users/baseb/convexity/vitna-code/docs/adr/ADR-0004-persistence-engine-and-actor-model.md)
- [IPC Transport and Wire Framing (ADR-0005)](file:///c:/Users/baseb/convexity/vitna-code/docs/adr/ADR-0005-ipc-transport-and-wire-framing.md)
- [Threat Model and Trust Boundaries](file:///c:/Users/baseb/convexity/vitna-code/docs/threat-model/THREAT_MODEL.md)
- [Complete Authority Inventory](file:///c:/Users/baseb/convexity/vitna-code/docs/AUTHORITY_INVENTORY.md)
- [Dependency and License Decision Matrix](file:///c:/Users/baseb/convexity/vitna-code/docs/DEPENDENCY_MATRIX.md)
- [Tier 1 Platform Guarantee Matrix](file:///c:/Users/baseb/convexity/vitna-code/docs/PLATFORM_MATRIX.md)
- [Competitor Dossier and Claim Mapping](file:///c:/Users/baseb/convexity/vitna-code/docs/COMPETITOR_DOSSIER.md)

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](file:///c:/Users/baseb/convexity/vitna-code/LICENSE) for details.
