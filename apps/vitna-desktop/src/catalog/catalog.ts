import snapshot from '../../../../catalog/models-dev.json';

/**
 * The pinned model catalog: what each provider sells, what it costs, and how
 * much context it takes. Written by scripts/update-catalog.mjs from the public
 * models.dev database, committed with the date it was taken.
 *
 * Three things this is not.
 *
 * It is not live. Nothing in this window fetches it, and the file changes only
 * when somebody runs the script and reads the diff. A page that shows a price
 * is showing the price as of `fetched_at`, which is why every surface that
 * prints one prints that date beside it.
 *
 * It is not the daemon's arithmetic. `UsageUpdated.cost_usd` is the daemon's
 * to compute and send, and `cost_known` is the daemon's to answer. This window
 * still prints "cost unknown" whenever the daemon says so, and it will not put
 * a number of its own in that place. `costUsd` below exists so that the price
 * table is written down once rather than twice, for the daemon to bill against
 * and for a later check to re-derive against.
 *
 * It is not a claim about a provider. models.dev is a community database under
 * MIT, and a row in it is somebody's reading of a public price page, carrying
 * no citation and no retrieval date of its own. It is good enough to show an
 * operator what a model costs before they pick it. It is not good enough to
 * settle a bill.
 */

/** United States dollars per one million tokens. */
export interface ModelRate {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

export interface CatalogModel {
  /** The name a request body carries, which is what `UsageUpdated.model_sku` reports. */
  sku: string;
  name: string;
  context_tokens: number;
  output_tokens: number;
  usd_per_mtok: ModelRate;
  tool_call: boolean;
  reasoning: boolean;
  release_date: string;
  last_updated: string;
}

/** A model the source carries that the catalog cannot show, and why not. */
export interface OmittedSku {
  sku: string;
  reason: string;
}

export interface CatalogProvider {
  id: string;
  name: string;
  /** Environment variable names the provider's own tooling reads. This window never touches one. */
  env: string[];
  models: CatalogModel[];
  /**
   * Kept by name rather than dropped. A model absent from the file reads as a
   * model that does not exist; one here reads as a model the source describes
   * too thinly to price, which is the true statement.
   */
  omitted_skus: OmittedSku[];
}

export interface Catalog {
  source: string;
  source_license: string;
  source_sha256: string;
  /** The day the snapshot was taken, ISO, as scripts/update-catalog.mjs stamped it. */
  fetched_at: string;
  generator: string;
  providers: CatalogProvider[];
}

/**
 * The one cast in this module. The shape is asserted against the real file by
 * catalog.test.ts, so a snapshot that stops matching fails a test rather than
 * reaching a component as a surprise.
 */
export const CATALOG = snapshot as Catalog;

export const FETCHED_AT = CATALOG.fetched_at;

export function providerIds(): string[] {
  return CATALOG.providers.map((p) => p.id);
}

/**
 * The provider's entry, or null when the catalog does not carry it. Null is a
 * real answer and the callers say so out loud: a provider with no prices under
 * it should read as one this snapshot does not price, never as one with
 * nothing to sell.
 */
export function findProvider(providerId: string): CatalogProvider | null {
  return CATALOG.providers.find((p) => p.id === providerId) ?? null;
}

export function modelsFor(providerId: string): CatalogModel[] {
  return findProvider(providerId)?.models ?? [];
}

/**
 * One model, or null. Null happens for real: the daemon's placeholder sku
 * (`claude-3-7-sonnet`, crates/daemon/src/server.rs) is old enough that the
 * current catalog does not carry it, and a model released after `fetched_at`
 * will not be here either. Both cases want the same answer, which is that this
 * window cannot price it, rather than a guess from a neighbouring row.
 */
export function findModel(providerId: string, sku: string): CatalogModel | null {
  return modelsFor(providerId).find((m) => m.sku === sku) ?? null;
}

/** The three counters `UsageUpdated` carries, and the three this prices. */
export interface TokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
}

/**
 * What a usage reading costs at catalog rates.
 *
 * This window does not print the result as a run's cost. It is here so the
 * daemon has one implementation to bill against, and so a later check can
 * re-derive a figure the daemon sent rather than take it on trust.
 *
 * The three counters are read as DISJOINT: `cached_tokens` is billed at the
 * cache-read rate and is not also inside `prompt_tokens`. events.proto declares
 * three flat counters and says nothing about whether they overlap, and the two
 * providers behind them disagree in practice, so this is an assumption rather
 * than a reading of the protocol. It is the assumption that cannot go negative,
 * which is why it is the one taken, and it is pinned by a test so that settling
 * the protocol the other way fails loudly here instead of quietly repricing
 * every cached run.
 */
export function costUsd(model: CatalogModel, usage: TokenUsage): number {
  const rate = model.usd_per_mtok;
  const micros =
    usage.prompt_tokens * rate.input + usage.cached_tokens * rate.cache_read + usage.completion_tokens * rate.output;
  return micros / 1_000_000;
}

/** "1M", "200K", "128K". Context windows are read at a glance or not at all. */
export function formatTokens(count: number): string {
  if (count >= 1_000_000) {
    const millions = count / 1_000_000;
    return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}M`;
  }
  if (count >= 1_000) {
    const thousands = count / 1_000;
    return `${Number.isInteger(thousands) ? thousands : thousands.toFixed(1)}K`;
  }
  return String(count);
}

/** "$5 / $25 per Mtok", in and out, which is the pair anybody comparing models reads first. */
export function formatRate(model: CatalogModel): string {
  return `$${formatUsd(model.usd_per_mtok.input)} / $${formatUsd(model.usd_per_mtok.output)} per Mtok`;
}

function formatUsd(amount: number): string {
  if (Number.isInteger(amount)) return String(amount);
  // Sub-cent rates are real at the cheap end of the table, so the decimals run
  // as far as the number does rather than rounding a price to $0.00.
  return amount < 0.01 ? amount.toPrecision(2) : amount.toFixed(2);
}
