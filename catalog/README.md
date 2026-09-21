# catalog

`models-dev.json` is what each model provider sells: the skus, what they cost,
how much context they take, and whether they can call a tool. It is a copy of
the public [models.dev](https://models.dev) database (MIT), narrowed to the
providers `crates/providers/` can actually call, taken on the date the file
records and committed.

```
node scripts/update-catalog.mjs             fetch, compare, write
node scripts/update-catalog.mjs --dry-run   fetch, compare, write nothing
```

The script prints what moved since the committed copy, one line per model added,
removed or repriced. Read that before committing: it is the reason the file is
in git rather than in a fetch.

## Why it is pinned

Nothing here is fetched at runtime, by anything, ever.

The desktop window promises that nothing it shows leaves the machine. It bundles
its own fonts, carries a `default-src 'self'` policy, and
`apps/vitna-desktop/scripts/check-bundle.mjs` fails the build over a third-party
host. A window that fetched prices at startup would be a different product than
the one that claim describes.

A receipt is also worth what its inputs are worth. A price that arrived from the
public internet at an unrecorded moment cannot be re-derived later; a file in git
with a date on it can. The upstream database moved twice in the three minutes
between the first two runs of the updater, which is the ordinary behaviour of a
live source and exactly what a signed artifact cannot be built on.

## What it is not

It is not a bill, and it is not evidence. models.dev is a community database:
a row in it is somebody's reading of a public price page, carrying no citation
and no retrieval date of its own. It is good enough to show an operator what a
model costs before they pick one. It is not good enough to settle an invoice, and
nothing in this repository should ever treat it as though it were.

It is not the daemon's arithmetic either. `UsageUpdated.cost_usd` is the
daemon's to compute and `cost_known` is the daemon's to answer. The window
prints "cost unknown" whenever the daemon says so and never puts a number of its
own in that place. `costUsd` in `apps/vitna-desktop/src/catalog/catalog.ts` is
here so the price table is written down once rather than twice: for the daemon to
bill against, and for a later check to re-derive a figure the daemon sent instead
of taking it on trust.

If this ever needs to be evidence rather than reference, the replacement already
exists and is ours: vitna.ai publishes a sourced, retrieval-dated, signed price
record at `/api/v1/snapshot`, no key required. Its coverage is narrower and its
provenance is real, which is the trade to make the day a receipt has to carry a
dollar figure somebody might dispute.

## Rules the file keeps

- **A row carries a price and a context window, or it is not a row.** Five
  OpenAI image models fail that today, four with no per-token price and one with
  no declared limits. They are listed by name under `omitted_skus` with the
  reason, because a model missing from the file entirely reads as a model that
  does not exist, while one named there reads as a model the source describes
  too thinly to price. That is the true statement and the one an operator can
  act on.
- **`fake` is in no catalog.** It invents its tokens and has no price.
  `apps/vitna-desktop/src/catalog/catalog.test.ts` reads the provider modules out
  of `crates/providers/src/lib.rs` and holds both halves of that rule: a new
  provider adapter with no catalog entry fails, and a catalog entry no adapter
  can call fails too.
- **Freshness is shown, not enforced.** Every surface that prints a price prints
  `fetched_at` beside it. No test fails because a Tuesday passed: a check that
  goes red on its own teaches everyone to ignore it, and whether a price is too
  old to trust is a judgement for whoever is reading the page.

## The open question

`events.proto` declares `prompt_tokens`, `completion_tokens` and `cached_tokens`
as three flat counters and never says whether the cached count sits inside the
prompt count. The two providers behind them disagree in practice. `costUsd`
reads them as disjoint, which is the reading that cannot go negative, and a test
pins it by name so that settling the protocol the other way fails loudly here
rather than quietly repricing every cached run.
