import { readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { CATALOG, costUsd, findModel, findProvider, formatRate, formatTokens, modelsFor } from './catalog';
import { SAMPLE_PROVIDERS } from '../sample/sampleTransport';

const repoFile = (path: string) => fileURLToPath(new URL(`../../../../${path}`, import.meta.url));

describe('the pinned catalog', () => {
  it('says where it came from and when', () => {
    expect(CATALOG.source).toBe('https://models.dev/api.json');
    expect(CATALOG.source_license).toBe('MIT');
    expect(CATALOG.generator).toBe('scripts/update-catalog.mjs');
    expect(CATALOG.source_sha256).toMatch(/^[0-9a-f]{64}$/);

    // A date that parses and is not in the future. Deliberately not "recent":
    // a test that goes red because a Tuesday passed teaches everyone to ignore
    // it, and the judgement about whether a price is too old to show belongs to
    // the operator reading `fetched_at` on the page, not to a clock in CI.
    expect(CATALOG.fetched_at).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    const taken = new Date(`${CATALOG.fetched_at}T00:00:00Z`);
    expect(Number.isNaN(taken.getTime())).toBe(false);
    expect(taken.getTime()).toBeLessThanOrEqual(Date.now());
  });

  it('carries the fields every surface reads, on every row', () => {
    expect(CATALOG.providers.length).toBeGreaterThan(0);
    for (const provider of CATALOG.providers) {
      expect(provider.id).toMatch(/^[a-z0-9-]+$/);
      expect(provider.name.length).toBeGreaterThan(0);
      expect(Array.isArray(provider.env)).toBe(true);
      expect(provider.models.length).toBeGreaterThan(0);

      // An omission says which sku and why. A bare list of skus would leave
      // the next reader guessing whether a model is missing because nobody
      // prices it or because the parse quietly stopped understanding it.
      for (const omitted of provider.omitted_skus) {
        expect(omitted.sku.length, provider.id).toBeGreaterThan(0);
        expect(omitted.reason.length, `${provider.id}/${omitted.sku} has no reason`).toBeGreaterThan(0);
        expect(
          provider.models.some((m) => m.sku === omitted.sku),
          `${provider.id}/${omitted.sku} is both priced and omitted`,
        ).toBe(false);
      }

      for (const model of provider.models) {
        const where = `${provider.id}/${model.sku}`;
        expect(model.sku.length, where).toBeGreaterThan(0);
        expect(model.name.length, where).toBeGreaterThan(0);
        expect(Number.isInteger(model.context_tokens), where).toBe(true);
        expect(model.context_tokens, where).toBeGreaterThan(0);
        expect(Number.isInteger(model.output_tokens), where).toBe(true);
        expect(model.output_tokens, where).toBeGreaterThan(0);
        expect(typeof model.tool_call, where).toBe('boolean');
        expect(typeof model.reasoning, where).toBe('boolean');

        // Every rate is a real number at or above zero. A null, a NaN or a
        // string here would render as "$NaN / $NaN per Mtok" on a page that
        // otherwise looks finished, which is the failure this row exists for.
        for (const [field, rate] of Object.entries(model.usd_per_mtok)) {
          expect(Number.isFinite(rate), `${where} ${field}`).toBe(true);
          expect(rate, `${where} ${field}`).toBeGreaterThanOrEqual(0);
        }
      }

      // Two models cannot share a sku: the sku is what UsageUpdated reports and
      // what a lookup resolves, so a duplicate makes the price depend on order.
      const skus = provider.models.map((m) => m.sku);
      expect(new Set(skus).size).toBe(skus.length);
    }
  });

  it('prices every provider the daemon can call, and exempts only the fake one', () => {
    // Read from the Rust source rather than restating its provider list here.
    // A second copy of the list would go on passing while the crate changed,
    // which is how a catalog ends up missing the provider somebody just added.
    const lib = readFileSync(repoFile('crates/providers/src/lib.rs'), 'utf8');
    const modules = new Set<string>();
    for (const line of lib.split('\n')) {
      const use = /^pub use (\w+)::(.+);$/.exec(line.trim());
      if (use && /\b[A-Z]\w*Provider\b/.test(use[2] ?? '')) modules.add(use[1] ?? '');
    }

    // If the selector above ever stops matching, this test would pass over an
    // empty set and prove nothing. `fake` is the one module guaranteed to be
    // there, so its absence means the selector broke rather than the catalog.
    expect(modules.size, 'no provider modules found in crates/providers/src/lib.rs').toBeGreaterThan(1);
    expect(modules.has('fake'), 'the fake provider module has moved or been renamed').toBe(true);

    // `fake` invents its tokens and has no price. It is the only exemption, and
    // it is exempt by name so that a second unpriced provider has to be argued
    // for here rather than added quietly.
    const callable = [...modules].filter((id) => id !== 'fake');
    for (const id of callable) {
      expect(findProvider(id), `crates/providers has ${id}, the catalog does not price it`).not.toBeNull();
    }
    for (const provider of CATALOG.providers) {
      expect(
        callable.includes(provider.id),
        `the catalog prices ${provider.id}, which no provider adapter can call`,
      ).toBe(true);
    }
    expect(findProvider('fake'), 'the fake provider has no price and belongs in no catalog').toBeNull();
  });

  it('prices every provider the window offers an operator', () => {
    // The other direction, against the host-facing list rather than the crate.
    // A provider can reach the providers dialog without a catalog entry, and
    // the failure then is silent: a name with an empty model list under it,
    // which reads as a provider that sells nothing.
    for (const provider of SAMPLE_PROVIDERS) {
      expect(modelsFor(provider.id).length, `${provider.id} is offered with no models under it`).toBeGreaterThan(0);
    }
  });

  it('is small enough to ship inside the window', () => {
    // It goes into dist/. This window bundles its own fonts and refuses a CDN,
    // so weight is a thing it pays for deliberately rather than by accident.
    const bytes = statSync(repoFile('catalog/models-dev.json')).size;
    expect(bytes).toBeLessThan(200_000);
  });
});

describe('looking a model up', () => {
  it('finds one by provider and sku', () => {
    const model = findModel('anthropic', 'claude-opus-5');
    expect(model?.name).toBe('Claude Opus 5');
    expect(model?.usd_per_mtok.input).toBeGreaterThan(0);
  });

  it('answers null for the sku the daemon hardcodes today', () => {
    // crates/daemon/src/server.rs reports `claude-3-7-sonnet`, which this
    // catalog does not carry. The answer is null rather than a neighbouring
    // Claude's rate: a window that cannot price a model says so.
    expect(findModel('anthropic', 'claude-3-7-sonnet')).toBeNull();
  });

  it('answers null for a provider it does not carry, and empty for its models', () => {
    expect(findProvider('nobody')).toBeNull();
    expect(modelsFor('nobody')).toEqual([]);
    expect(findModel('nobody', 'claude-opus-5')).toBeNull();
  });
});

describe('pricing a usage reading', () => {
  const model = {
    sku: 'test',
    name: 'Test',
    context_tokens: 1000,
    output_tokens: 100,
    usd_per_mtok: { input: 5, output: 25, cache_read: 0.5, cache_write: 6.25 },
    tool_call: true,
    reasoning: false,
    release_date: '',
    last_updated: '',
  };

  it('bills input, output and cache reads at their own rates', () => {
    const cost = costUsd(model, { prompt_tokens: 1_000_000, completion_tokens: 1_000_000, cached_tokens: 0 });
    expect(cost).toBeCloseTo(30, 10);
  });

  it('reads the three counters as disjoint, which is an assumption and not the protocol', () => {
    // events.proto declares prompt, completion and cached as three flat
    // counters and never says whether cached sits inside prompt. This pins the
    // reading catalog.ts documents: 1M cached tokens cost the cache-read rate
    // ON TOP of the prompt tokens, they are not a discount inside them.
    // Settling the protocol the other way must fail here, loudly, rather than
    // silently reprice every cached run in the product.
    const cost = costUsd(model, { prompt_tokens: 0, completion_tokens: 0, cached_tokens: 1_000_000 });
    expect(cost).toBeCloseTo(0.5, 10);

    const both = costUsd(model, { prompt_tokens: 1_000_000, completion_tokens: 0, cached_tokens: 1_000_000 });
    expect(both).toBeCloseTo(5.5, 10);
  });

  it('costs nothing when nothing ran', () => {
    expect(costUsd(model, { prompt_tokens: 0, completion_tokens: 0, cached_tokens: 0 })).toBe(0);
  });
});

describe('formatting', () => {
  it('reads a context window at a glance', () => {
    expect(formatTokens(1_000_000)).toBe('1M');
    expect(formatTokens(200_000)).toBe('200K');
    expect(formatTokens(128_000)).toBe('128K');
    expect(formatTokens(400)).toBe('400');
  });

  it('keeps a sub-cent rate legible instead of rounding it to nothing', () => {
    const cheap = {
      sku: 'cheap',
      name: 'Cheap',
      context_tokens: 1000,
      output_tokens: 100,
      usd_per_mtok: { input: 0.05, output: 0.4, cache_read: 0.005, cache_write: 0.05 },
      tool_call: false,
      reasoning: false,
      release_date: '',
      last_updated: '',
    };
    expect(formatRate(cheap)).toBe('$0.05 / $0.40 per Mtok');
    expect(formatRate({ ...cheap, usd_per_mtok: { ...cheap.usd_per_mtok, input: 0.002 } })).toBe(
      '$0.0020 / $0.40 per Mtok',
    );
  });
});
