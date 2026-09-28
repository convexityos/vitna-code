# ADR-0006: The Browser Interface over Loopback (`vitna app`)

- Status: Proposed
- Date: 2026-09-28
- Deciders: the owner, on 2026-09-28 (convexityos/vitna `APPS.md` sections 4.4 and 4.5)
- Relates to: ADR-0002 (execution boundary), ADR-0003 (sensitive storage), ADR-0005 (IPC transport)

## Context

On 2026-09-28 the owner moved Build's editing half into the browser. Code, a mode of the app at app.vitna.ai, opens a folder and has a model read it and propose each edit as a diff. It writes nothing until the person approves, and it runs nothing.

The machine half was recorded as arriving later, as "a local runner: the Rust daemon, which already carries the tools, the sandbox and the receipts, serving this same interface from the person's own machine, so app.vitna.ai keeps its openrouter.ai-only boundary." Later that day the owner picked that runner as the desktop app. The alternatives were a native shell around the page and resuming the native window (#5).

What the runner adds is what only a machine can do:

- read and write the folder in place, in any browser, without the browser's folder picker;
- run a command, such as the project's tests, inside the OS sandbox;
- sign each run with the device key the account keeps (#29).

A browser reaches a local process in one way: HTTP. ADR-0005 decided that the daemon "never binds to loopback TCP (`127.0.0.1`) by default, eliminating attacks from local browsers via WebSockets, DNS rebinding, or CORS exploits". `crates/protocol/src/endpoint.rs` holds every declared IPC endpoint to that.

This ADR does not change ADR-0005. It adds a second, separate listener, started only by an explicit command, and answers each of those attacks by name.

## Decision

### 1. A separate listener, never the daemon's IPC

`vitna app` starts an HTTP/1.1 server in its own process (`crates/app-server`).

- `vitna-coded` and `vitna serve` never bind TCP.
- The IPC endpoints ADR-0005 declares are unchanged, so `no_declared_endpoint_is_tcp` still holds.

### 2. Opt-in, per launch, on the loopback only

- **The origin is `http://localhost:<port>`.** That is the name OpenRouter documents for a local app's key callback, "supported on any port" (openrouter.ai/docs, OAuth PKCE guide, retrieved 2026-09-28). The guide says nothing about `127.0.0.1`, and the page connects the person's key through that flow.
- **Both loopback addresses are held.** A browser may look `localhost` up as `[::1]` before `127.0.0.1`.
  - A server holding only one address would let another process, another account's included, take the other on the same port.
  - That process would then serve its own page at the very origin this one uses, and could read the launch code out of the address.
  - So the server binds `127.0.0.1` and, where the machine has an IPv6 loopback, `[::1]`, on one port.
  - Either address being taken is an error. A machine with no IPv6 loopback has no `[::1]` for anyone to take.
- **Never a wildcard address.**
- **The port.** It is 7788 unless `--port` says otherwise.
- **A port in use is an error, not a reason to move.** The page keeps the person's key and conversations in its origin's storage, and a moved port is a different origin.

### 3. Admission, in this order, before any handler runs

1. **The TCP peer** must be a loopback address.
2. **The `Host` header** must be exactly `localhost:<port>`, `127.0.0.1:<port>` or `[::1]:<port>`. A page on another name that resolves to a loopback address (DNS rebinding) sends its own name, and is refused. This applies to the interface's files as well as the API.
3. **The request's origin.** Every API request is a `POST` and must carry an `Origin` equal to `http://` plus the `Host` it was sent to. A browser that sends `Sec-Fetch-Site` must send `same-origin`. Browsers send `Origin` on every cross-site request and on a same-origin POST, so a request from any other page is refused.
4. **The session token.** Every API request except the session exchange must carry `Authorization: Bearer <session token>`.
   - The server sends no CORS headers at all.
   - So a cross-origin page cannot get past the preflight that a custom header forces, and could not read a reply if it did.
5. **Nothing else.** There is no WebSocket upgrade. No route answers anything but `GET`/`HEAD` for the interface's files and `POST` for the API.
6. **No framing.** Every reply for the interface forbids framing (`Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY`), so another site cannot frame the page and steer a person's clicks on it. The page's own `<meta>` policy cannot say this.

### 4. Session bootstrap without a lasting secret on a command line

1. `vitna app` mints a one-time launch code: 32 random bytes, valid for 120 seconds and for one use.
2. It opens `http://localhost:<port>/#launch=<code>` as an app window of the person's own Chromium browser (`--app`), or in the default browser where there is none.
3. The code travels in the fragment, which a browser never sends to any server.
4. The page exchanges it at `POST /api/v1/session` for a session token and removes it from the address bar. The token is 32 random bytes, held only in the server's memory and the tab's `sessionStorage`.

A code read from a process list is useless once used or expired. Pressing Enter in the terminal mints a fresh one for another window. `--no-open` prints the address instead of opening it.

### 5. One folder per server, behind two layers of guard

**The tools' file layer.** Every path goes through the tools crate's own file layer, `vitna_tools::workspace_files`, which wraps the code `read_file`, `write_file` and `apply_patch` use:

- it resolves on the real filesystem and stays inside the root;
- it opens regular files only;
- it captures a preimage from the handle it writes through;
- it reads each write back.

**The page's rules, held again on the server.** On top of that the server refuses what the page refuses. Its rules are a copy of the page's `resolvePath` and `blocked`:

- no absolute paths, drives or `..`;
- no names ending in a dot or a space, and no colons;
- nothing inside `.git`, `node_modules` or `.vitna`, at any depth and in any case;
- no files that usually hold secrets.

The server checks those rules twice: on the path it was given, and on where that path leads once links and aliases are resolved. A link inside the folder can reach `.git/config`, and on Windows the short name `GIT~1` is `.git`.

The page's guards are a courtesy to the person; the server's are the boundary.

### 6. Writes are compare-and-swap

- **Writes.** A write names the SHA-256 the file must hold now, or `absent` for a new file. A mismatch is refused with the file's current hash, and nothing is written.
- **Removes.** Only undoing a new file uses a remove, and it names the hash too.
- **Removing the right file.** A remove deletes the very file it hashed, never one put in its place since:
  - on Windows, through the handle it hashed;
  - elsewhere, by name inside a folder handle it verified, after checking that the name still leads to that file.

### 7. Commands only through the runner and the sandbox

- **The runner.** `run` goes to `vitna_runner` with the same `CommandRequest` a tool builds. The sandbox, the cleared environment, the timeout and the process-tree kill are therefore the runner's, unchanged.
- **One at a time.** Commands run one after another.
- **No sandbox.** Where no sandbox is available (Windows today, or Linux without bubblewrap), a command is refused. The exception is a server the operator started with `--allow-unsandboxed`. That decision is made at the terminal, never by the page. The page is told, before anything runs, which of the two it is.
- **No network** inside the sandbox.

### 8. Receipts sign what the server saw, and say what it only heard

**The journal.** The server keeps a journal for each session, meaning each window. It records the effects the session caused since its last receipt:

- every write and remove, with the hashes read from disk;
- every command, with the action id and statement digest the runner produced and the isolation it ran under;
- every command that did not run or did not finish, with the reason.

**The receipt.** `POST /api/v1/receipt` builds a `vitna-run-receipt-v1` from that journal. It signs it with the device key and writes it to `<folder>/.vitna/receipts/<run>.json`, through the same file layer.

- `event_hash_chain_root` is the Merkle root of a hash chain over the journal. Each effect is hashed with the hash before it, so the root commits to their order.
- `changeset` has one entry per file, from its state before the first change to its state after the last. `diff_digest` covers each path with the hashes of its two sides, since the server never computes a diff.
- Each runner statement carries this device's signature over its statement digest. It names the runner's own action id, so anyone holding the output can recompute the digest.
- `base_commit_sha` is read from the folder's `.git` when it has one.
- The model and its provider are recorded as the page reported them, and `routing_reason` says `reported_by_page`, since the runner never sees a model.
- The completion state is never better than `completed_with_unknowns`, because the runner cannot judge whether the work is done. The page may report `cancelled`, `failed` or `blocked` instead.
- The isolation label is `full_access` the moment one command ran with no sandbox, and `guarded` otherwise.

### 9. The page it serves is the page app.vitna.ai serves, built for the runner

- **Where it comes from.** `--ui` names the built files, from `npm run chat:build -- --mode runner` in convexityos/vitna. Without it, `vitna app` looks for a `ui` folder beside itself, and failing that serves a page saying how to build one.
- **What it may connect to.** The build's own Content-Security-Policy allows its own origin and openrouter.ai only.
- **Who calls the model.** Model calls still go from the page to OpenRouter, on the person's own key. The runner holds no provider key and calls no model.

## Consequences

### Positive

- The same interface on both hosts, so there is one front end to keep correct.
- Real file access and sandboxed commands in any browser, including those without the File System Access API.
- Receipts that verify against a key that stays put.

### Negative and trade-offs

- **A loopback port exists while `vitna app` runs.**
  - Every attack ADR-0005 names is answered above, but any local process can see the port.
  - A process running as this account could read the session token from the browser's profile. That is the same trust the account already places in its own processes.
- **The launch code is visible in the process list** while the browser starts. It is single-use and short-lived for that reason.
- **The command environment is narrow.** The runner's command environment is the tool's cleared environment: `PATH` and, on Windows, the system variables. A test runner that needs `HOME` or `APPDATA` fails inside it until the environment policy is widened, which needs a decision of its own.
- **Command output is buffered whole.** The runner buffers a command's whole output, as the tool always has. The server returns the last 256 KB of each stream to the page and says what it cut, but it does not cap what the runner holds.
- **Unreceipted effects are lost on exit.** A session's journal lives in the server's memory. A window closed before asking for a receipt leaves its writes on disk and no receipt for them.
