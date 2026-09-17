// Refreshes catalog/models-dev.json, the pinned model catalog.
//
// The catalog is a copy, taken on a date, of the public models.dev database
// (MIT, https://github.com/sst/models.dev). It is committed rather than
// fetched at runtime for three reasons:
//
//   1. The desktop window promises that nothing it shows leaves the machine.
//      It carries a `default-src 'self'` policy and apps/vitna-desktop/
//      scripts/check-bundle.mjs fails the build over a third-party host, so a
//      window that fetched prices at startup would be a different product.
//   2. A receipt is worth what its inputs are worth. A price that arrived from
//      the public internet at an unrecorded moment cannot be re-derived later;
//      a file in git with a date on it can.
//   3. A price change should be a diff somebody reads, not a number that moves
//      under a running window. That is what the summary below is for.
//
// Usage:
//   node scripts/update-catalog.mjs             fetch, compare, write
//   node scripts/update-catalog.mjs --dry-run   fetch, compare, write nothing
//
// It writes nothing at all when anything is off: a non-200, a short body, a
// declared provider upstream has dropped, a provider with no models. A catalog
// half-written from a bad fetch is worse than yesterday's catalog, because
// yesterday's was true on a day we can name.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SOURCE_URL = 'https://models.dev/api.json';
const SOURCE_LICENSE = 'MIT';

// The providers vitna-coded can actually call, which is crates/providers/src/
// minus `fake`. `fake` invents its tokens, so it has no price and belongs in
// no catalog; apps/vitna-desktop/src/catalog/catalog.test.ts holds both halves
// of that rule against the Rust source, so adding a provider adapter without
// adding it here fails a test rather than showing an empty list to an operator.
const PROVIDERS = ['anthropic', 'openai'];

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const target = join(root, 'catalog', 'models-dev.json');
const dryRun = process.argv.includes('--dry-run');

function die(message) {
  console.error(`update-catalog: ${message}`);
  console.error('update-catalog: nothing written.');
  process.exit(1);
}

const response = await fetch(SOURCE_URL, { headers: { accept: 'application/json' } }).catch((e) =>
  die(`could not reach ${SOURCE_URL}: ${e instanceof Error ? e.message : String(e)}`),
);
if (!response.ok) die(`${SOURCE_URL} answered ${response.status}`);

const body = await response.text();
// The real document is several megabytes. Anything small enough to be an error
// page is not the database, whatever status code came with it.
if (body.length < 100_000) die(`${SOURCE_URL} returned ${body.length} bytes, which is too small to be the database`);

const sha256 = createHash('sha256').update(body).digest('hex');

let upstream;
try {
  upstream = JSON.parse(body);
} catch (e) {
  die(`the body is not JSON: ${e instanceof Error ? e.message : String(e)}`);
}

/**
 * models.dev spells prices as USD per million tokens under `cost`, and limits
 * under `limit`. Both names are renamed on the way in so the unit is written
 * down where the number is: a bare `input: 5` invites somebody to read it as
 * dollars per call a year from now.
 *
 * Returns either a catalog row or a string saying why there is not one. A row
 * has to carry a price AND a context window to be worth showing: the image
 * models upstream carry neither or only one, and a zero written into either
 * field renders as "0 tokens" on a page that otherwise looks finished.
 */
function readModel(sku, model) {
  const cost = model.cost;
  const limit = model.limit ?? {};
  if (!cost || typeof cost.input !== 'number' || typeof cost.output !== 'number') return 'no per-token price';
  if (!(Number(limit.context) > 0)) return 'no context limit';
  if (!(Number(limit.output) > 0)) return 'no output limit';
  return {
    sku,
    name: typeof model.name === 'string' ? model.name : sku,
    context_tokens: Number(limit.context ?? 0),
    output_tokens: Number(limit.output ?? 0),
    usd_per_mtok: {
      input: cost.input,
      output: cost.output,
      // A provider with no cache tier prices a cache read at its input rate,
      // which is what not having a cache costs you.
      cache_read: typeof cost.cache_read === 'number' ? cost.cache_read : cost.input,
      cache_write: typeof cost.cache_write === 'number' ? cost.cache_write : cost.input,
    },
    tool_call: model.tool_call === true,
    reasoning: model.reasoning === true,
    release_date: typeof model.release_date === 'string' ? model.release_date : '',
    last_updated: typeof model.last_updated === 'string' ? model.last_updated : '',
  };
}

const providers = PROVIDERS.map((id) => {
  const provider = upstream[id];
  if (!provider) die(`${SOURCE_URL} no longer carries the provider "${id}"`);
  const entries = Object.entries(provider.models ?? {});
  if (entries.length === 0) die(`${SOURCE_URL} carries no models for "${id}"`);

  const models = [];
  const omitted = [];
  for (const [sku, model] of entries) {
    const read = readModel(sku, model);
    // A row that cannot be shown is kept by name, with the reason it cannot.
    // A model missing from the file entirely reads as a model that does not
    // exist; a model in `omitted_skus` reads as one the source describes too
    // thinly to price here, which is the true statement and the one an
    // operator can act on. It is also the difference between noticing that a
    // whole provider went unreadable and never hearing about it.
    if (typeof read === 'string') omitted.push({ sku, reason: read });
    else models.push(read);
  }
  models.sort((a, b) => a.sku.localeCompare(b.sku));
  omitted.sort((a, b) => a.sku.localeCompare(b.sku));

  // Every row of a provider being unusable is not a catalog, it is a parse
  // that stopped working. The fetch is refused rather than committed.
  if (models.length === 0) die(`every model under "${id}" was omitted; the upstream shape has probably changed`);

  return {
    id,
    name: typeof provider.name === 'string' ? provider.name : id,
    // The environment variables the provider's own SDKs read. The window never
    // touches a key; this is here so a page can say which name to set.
    env: Array.isArray(provider.env) ? [...provider.env] : [],
    models,
    omitted_skus: omitted,
  };
});

const next = {
  source: SOURCE_URL,
  source_license: SOURCE_LICENSE,
  source_sha256: sha256,
  // The day, not the second. A catalog refreshed twice in one afternoon should
  // produce one diff about prices, not two about clocks.
  fetched_at: new Date().toISOString().slice(0, 10),
  generator: 'scripts/update-catalog.mjs',
  providers,
};

let previous = null;
try {
  previous = JSON.parse(readFileSync(target, 'utf8'));
} catch {
  previous = null;
}

function summarise(before, after) {
  if (!before) return ['first catalog: nothing to compare against'];
  const lines = [];
  for (const provider of after.providers) {
    const old = (before.providers ?? []).find((p) => p.id === provider.id);
    const oldModels = new Map((old?.models ?? []).map((m) => [m.sku, m]));
    const newModels = new Map(provider.models.map((m) => [m.sku, m]));
    for (const sku of newModels.keys()) if (!oldModels.has(sku)) lines.push(`+ ${provider.id}/${sku}`);
    for (const sku of oldModels.keys()) if (!newModels.has(sku)) lines.push(`- ${provider.id}/${sku}`);
    for (const [sku, model] of newModels) {
      const was = oldModels.get(sku);
      if (!was) continue;
      for (const field of ['input', 'output', 'cache_read', 'cache_write']) {
        const before$ = was.usd_per_mtok?.[field];
        const after$ = model.usd_per_mtok[field];
        if (before$ !== after$) lines.push(`~ ${provider.id}/${sku} ${field}: ${before$} -> ${after$}`);
      }
      if (was.context_tokens !== model.context_tokens) {
        lines.push(`~ ${provider.id}/${sku} context: ${was.context_tokens} -> ${model.context_tokens}`);
      }
    }
  }
  return lines.length ? lines : ['no model, price or context change'];
}

const counts = next.providers.map((p) => `${p.id} ${p.models.length}`).join(', ');
console.log(`update-catalog: ${SOURCE_URL} ${body.length} bytes, sha256 ${sha256.slice(0, 12)}`);
console.log(`update-catalog: ${counts}`);
for (const line of summarise(previous, next)) console.log(`  ${line}`);

if (dryRun) {
  console.log('update-catalog: --dry-run, nothing written.');
} else {
  writeFileSync(target, `${JSON.stringify(next, null, 2)}\n`, 'utf8');
  console.log(`update-catalog: wrote ${target}`);
}
