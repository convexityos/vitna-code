# vitna-desktop

The Vitna Code desktop frontend: a session as a transcript, the approvals it
asks for, the diff it wants to write, and the receipt it leaves. Vite, React
18, TypeScript, no other runtime dependencies.

```
npm ci
npm run dev        # http://localhost:5183
npm test           # vitest
npm run build      # tsc, vite build, then scripts/check-bundle.mjs
```

## What it talks to

The daemon, `vitna-coded`, listens on an owner-only Unix socket or Windows
named pipe and never on loopback TCP (ADR-0005). A web page cannot open
either, on purpose, so this app reaches the daemon only through a bridge
object a desktop shell places on `window` before the page loads:
`src/transport/bridge.ts` declares that contract (`window.vitnaBridge`,
`apiVersion: 1`, bytes in and bytes out). In a plain browser the bridge is
absent and the app says so rather than pretending.

Everything above the bridge is written against the Vitna Agent Protocol as
DECLARED in `protocol/vitna/protocol/v1/*.proto`:

- `src/protocol/types.ts` mirrors the messages field for field.
- `src/protocol/codec.ts` decodes payloads with proto3 semantics: a missing
  field takes its default, a wrong type is an error, never a guess.
- `src/protocol/frame.ts` frames the way `crates/protocol` does today (4-byte
  length, then serde JSON), and says so, because the ADR asks for protobuf and
  the code does not do that yet. One file to change when the daemon settles it.
- `src/run/reducer.ts` folds the event stream into a view, in sequence order,
  and asks again after a gap instead of applying out of order.
- `src/client/client.ts` does the handshake and subscription.

## What it will not do

- Decide for the operator. An approval is a box in the transcript with
  numbered choices. `y`/`n`/digits work only while the box has focus, a held
  key does not repeat, and a decision that has left the window cannot be
  changed by any later key or click. "Approved" is printed only when the
  daemon reports the tool starting.
- Show a number it did not receive. Cost prints only when the daemon says it
  knows it; a model name comes from `UsageUpdated`, never from this app. The
  providers dialog does list what each model costs, from the pinned catalog in
  `../../catalog/` by way of `src/catalog/`, and that is a price list with the
  date it was taken printed under it, not a claim about your run.
- Call it verified when it is not. The receipt reader checks an Ed25519
  signature with WebCrypto against a key the reader supplies, and says "not
  checked" without one. The Rust verifier prints PASSED in that case; this
  app does not.
- Load anything from the network. Fonts are bundled, the production page
  carries a `default-src 'self'` policy, and `scripts/check-bundle.mjs`
  fails the build if a third-party host or the sample session reaches `dist/`.

## The sample session

`src/sample/` is a scripted daemon for development: it speaks real envelopes
through the real client and reducer, gates on a real SHA-256 digest, refuses
a wrong one, and never runs anything. It is reached only through a dynamic
import behind `import.meta.env.DEV`, so production builds drop it, and the
bundle check proves that. Open it from the no-daemon page or with `/?sample`.

## Design

Vitna's console material (the `vitna` repo, `DESIGN_GUARDRAILS.md` and
`app/styles/console.css`), read as a terminal: one column, a glyph gutter, the
model after a dot, you after `❯`, a tool as one line with its result under a
`⎿`, a decision as a numbered box. Periwinkle for what the window asks of you
and where you stand, rust for what waits on you, green as one dot beside
"connected" and nothing else. No glass, no glow, no emoji, and no em-dashes
anywhere in this tree.

## Known gaps on the daemon side

As of 2026-09-17 the Rust workspace does not compile (missing workspace
dependencies), `vitna-coded` prints one line and exits, approvals abort the
run rather than wait, and receipts are signed with a key the daemon discards.
This app is built to the declared protocol so that fixing those needs no
change here; the bridge is the one piece still to write on the shell side.
