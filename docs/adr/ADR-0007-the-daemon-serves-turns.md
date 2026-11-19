# ADR-0007: The Daemon Serves Turns, through a Runtime Lane

- Status: Proposed
- Date: 2026-10-19
- Deciders: pending the owner's review of the pull request that carries it
- Relates to: ADR-0001 (invariants), ADR-0002 (execution boundary), ADR-0005 (IPC transport), #22 (declared event payloads, step 3c-i, which this builds on), convexityos/vitna `STACK.md` layer 3

## Context

On `main`, no model was ever asked anything. `vitna run` matched "create" or "edit" in the prompt and wrote a fixed file, and `vitna-coded` answered the handshake and `SubscribeEvents` and refused every other declared command. The loop that asks a model and runs the tools it picks existed only on `native-desktop-app` (#5), driven by a scripted provider, behind wire names nobody declared (`SubmitTurnRequest`, `TurnResult`), which `tests/declared_protocol_conformance.rs` was written to stop.

`STACK.md` (convexityos/vitna) set the order: the loop lands on `main` "with its provider pointed at a runtime lane", so every model call Vitna Code makes is routed and ledgered by `vitna-runtime` rather than by a second client holding provider keys.

## Decision

### 1. The loop lands, on the declared wire

`crates/orchestration/src/agent.rs` carries #5's loop. Every tool call the model makes goes through `OrchestrationEngine::execute_tool`, the path `execute_task_step` now delegates to, so a scripted step and a model's call meet one approval gate, one event chain and one evidence capture.

It builds on #22, which made every path record `ApprovalRequested`, `ToolStarted` and `ToolFinished` with their declared payloads, linked by one `tool_call_id`, and gave the engine's own records a `vitna.audit.v1.` prefix behind a deny-by-default `record_event`. On a model's call the `tool_call_id` is the model's own id, so a client links the model's proposal to its approval, start and finish, and the call also announces `ToolProposed`, `DiffChanged`, `MessageDelta`, `UsageUpdated`, `RunStateChanged` and `Diagnostic`. Three audit names join #22's four: `ApprovalRejected`, `ModelRequested` and `ModelResponded`. A tool that started and then errored now always records its `ToolFinished`, as `failed`, on every path.

`vitna-coded` serves four more commands: `CreateSession`, `SubmitTurn`, `ApproveAction` and `RejectAction`.

### 2. One addition to the protocol: `SessionCreated`

`CreateSession` had no declared reply, so a client could create a session and never learn its id: nothing else in the protocol names a session before its first event. `envelope.proto` gains `SessionCreated { session_id, workspace_path, initial_mode }`, sent once on the connection that asked, at control sequence 0, beside `HandshakeResponse` and `ErrorResponse`.

Migration: additive. No client sent `CreateSession` before this, since no daemon served it. `apps/vitna-desktop/src/protocol/types.ts` and `codec.ts` decode the reply, and the desktop reducer keeps it out of the event stream as it does the handshake. The protocol version stays 1.0; a client meeting an older daemon gets the existing "declared but not served yet" refusal.

`SubmitTurn` has no reply either, and gets none. The turn's events on the session's subscription are the answer, beginning with the run moving to `running_model`, and a refusal is an `ErrorResponse`. A client subscribes before it submits.

### 3. The model is a runtime lane, fixed when the daemon starts

`vitna-coded --runtime-url http://127.0.0.1:8787 --lane vitna/code`, and the same two flags on `vitna serve`. Both or neither: one without the other is refused, and a daemon started with neither refuses `SubmitTurn` by naming the flags. There is no default lane, because a default is a model nobody chose.

Loopback only. A runtime on loopback asks for no key; one anywhere else needs `VITNA_INBOUND_KEY`, which CONTRIBUTING's fourth rule forbids reading from the environment and which the keychain cannot hold (it keys providers by `ProviderId`, which names no runtime). A remote address is refused when the route is formed rather than failing on the first turn with a 401 that reads like a bad provider key.

The receipt records the provider as `vitna-runtime` and the model as the one each response says served it, not the lane alias asked for. When responses named several models (a lane that fell back mid-turn), the receipt names all of them and its routing reason says so, since naming one would be a silent model switch (ADR-0001 invariant 6). When none was named, it keeps the alias and says the served model was not reported.

### 4. Approvals wait for a person, bound to a digest

A turn that reaches a mutating action announces `ApprovalRequested` and waits. `ApproveAction` answers it only when its `action_digest` equals the one announced; a mismatch is refused with `ERROR_CODE_POLICY_DENIED`, pinned to the approval by `action_id`, and the request keeps waiting. `RejectAction` refuses it, and the model is told why. A request nobody answers is refused after `timeout_ms` (15 minutes, declared in the request), so a run whose operator walked away ends.

`ApprovalRequested.timeout_ms` is the deadline the request is held to, which is how `codec.ts` reads it ("zero is proto3's unset, which this client reads as no deadline given"). #22 wrote the action's own time limit there; that limit is part of the action and stays bound by the digest. A request nobody waits on, on the rule path, declares 0.

The unsandboxed-execution approval #22 kept as a second, separate request is put to the operator as its own question when an approver is present: approving it runs that one command with no OS sandbox, and refusing it refuses the command.

Only scope `once` is served (an empty scope reads as once). `session` and `project` are refused, because each would approve actions nobody has seen yet; the request keeps waiting for a `once`.

### 5. What a turn is when it ends

A turn ends `completed` when the model stops asking for tools, and `failed` when the provider fails, the round limit (24) is reached, or the receipt cannot be signed. The receipt is signed in every case, since the rounds before a failure may already have changed the folder; a stopped run is signed `failed`, never `completed_with_evidence`, and a round-limit stop carries an evidence item saying so. No receipt schema change was needed.

## Not decided here, and not built

- `initial_mode` is echoed and not applied: every mutating action is asked about whatever mode was named.
- `attached_file_paths` is refused rather than read.
- `SteerRun`, `PauseRun`, `ResumeRun` and `CancelRun` are still declared and not served, so a running turn cannot be stopped except by refusing its approvals or stopping the daemon.
- A session's conversation is held in memory: a restarted daemon starts every conversation afresh, though the journal keeps every event.
- A turn is one non-streaming call per round, so `MessageDelta` carries a round's whole text rather than token deltas.
- The terminal client and the desktop window do not send `SubmitTurn` yet.
- The action digest still binds tool and arguments only, not the executable, cwd, environment names or mounts that CONTRIBUTING's third rule lists; that is unchanged by this ADR.
