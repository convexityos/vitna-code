/**
 * vitna-run-receipt-v1: reading one, and rebuilding the exact bytes its
 * device signature covers.
 *
 * Two authorities describe a receipt and they do not agree, so both are
 * checked and both verdicts are shown:
 *
 * - schemas/vitna-run-receipt-v1.json, which makes evidence_items, changeset
 *   and runner_execution_statements optional.
 * - the Rust struct in crates/receipts/src/lib.rs, which the verifier
 *   deserializes into, where those three have no serde default and are
 *   therefore required. A schema-valid file without them is one
 *   vitna-receipt-verify refuses to load.
 *
 * The signed bytes follow VitnaRunReceiptV1::to_canonical_bytes: the struct
 * re-serialized with device_signature emptied, keys sorted at every level,
 * no whitespace. The struct decides what exists, so an Option that is None is
 * left out, an empty child_receipt_roots is left out, and any key the struct
 * does not declare is dropped before signing. That last point is why this
 * module lists uncovered keys: text in a receipt that no signature covers is
 * the thing a reader most needs to be told about.
 */

export const RECEIPT_SCHEMA_VERSION = 'vitna-run-receipt-v1';

export const ISOLATION_LABELS = ['read_only', 'guarded', 'strong', 'full_access'] as const;
export const COMPLETION_STATES = [
  'completed_with_evidence',
  'completed_with_unknowns',
  'blocked',
  'failed',
  'cancelled',
  'needs_reconciliation',
  'stopped_at_round_limit',
] as const;
/** Weakest to strongest, as the schema lists them. */
export const EVIDENCE_GRADES = [
  'model_reported',
  'broker_observed',
  'sandbox_captured',
  'independently_reproduced',
  'remote_or_hardware_attested',
] as const;

export interface ModelSelection {
  provider: string;
  model_sku: string;
  routing_reason: string;
  policy_digest: string | null;
}

export interface EvidenceItem {
  evidence_id: string;
  grade: string;
  description: string;
  artifact_digest: string | null;
}

export interface FileModification {
  path: string;
  preimage_hash: string;
  postimage_hash: string;
}

export interface ChangeSet {
  files_modified: FileModification[];
  diff_digest: string;
}

export interface RunnerStatement {
  action_id: string;
  statement_digest: string;
  signature: string;
}

export interface Receipt {
  schema_version: string;
  run_id: string;
  session_id: string;
  workspace_fingerprint: string;
  base_commit_sha: string;
  model_selection: ModelSelection;
  event_hash_chain_root: string;
  isolation_label: string;
  completion_state: string;
  evidence_items: EvidenceItem[];
  changeset: ChangeSet;
  runner_execution_statements: RunnerStatement[];
  child_receipt_roots: string[];
  device_signature: string;
}

export interface ReceiptReading {
  /** Null only when the text is not a JSON object at all. */
  receipt: Receipt | null;
  /** Breaks schemas/vitna-run-receipt-v1.json. */
  schemaErrors: string[];
  /** Fields the Rust struct requires that this file omits: vitna-receipt-verify would not load it. */
  rustMissing: string[];
  /** Keys the struct does not declare. They are dropped before signing, so no signature covers them. */
  uncovered: string[];
}

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj => v !== null && typeof v === 'object' && !Array.isArray(v);

/** True when a string holds an unpaired surrogate, which serde_json refuses to read. */
function hasLoneSurrogate(s: string): boolean {
  for (let i = 0; i < s.length; i += 1) {
    const c = s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff) {
      const d = s.charCodeAt(i + 1);
      if (!(d >= 0xdc00 && d <= 0xdfff)) return true;
      i += 1;
    } else if (c >= 0xdc00 && c <= 0xdfff) {
      return true;
    }
  }
  return false;
}

interface Reader {
  schemaErrors: string[];
  rustMissing: string[];
  uncovered: string[];
}

function readString(r: Reader, o: Obj, key: string, path: string, opts: { schemaRequired: boolean; rustRequired: boolean }): string {
  const v = o[key];
  const where = `${path}${key}`;
  if (v === undefined) {
    if (opts.schemaRequired) r.schemaErrors.push(`${where} is required`);
    if (opts.rustRequired) r.rustMissing.push(where);
    return '';
  }
  if (typeof v !== 'string') {
    r.schemaErrors.push(`${where} must be a string`);
    return '';
  }
  if (hasLoneSurrogate(v)) r.schemaErrors.push(`${where} is not valid Unicode`);
  return v;
}

function readOptionalString(r: Reader, o: Obj, key: string, path: string): string | null {
  const v = o[key];
  if (v === undefined || v === null) return null;
  if (typeof v !== 'string') {
    r.schemaErrors.push(`${path}${key} must be a string`);
    return null;
  }
  if (hasLoneSurrogate(v)) r.schemaErrors.push(`${path}${key} is not valid Unicode`);
  return v;
}

function noteUncovered(r: Reader, o: Obj, declared: readonly string[], path: string) {
  for (const key of Object.keys(o)) {
    if (!declared.includes(key)) r.uncovered.push(`${path}${key}`);
  }
}

function readList<T>(
  r: Reader,
  o: Obj,
  key: string,
  path: string,
  rustRequired: boolean,
  item: (value: unknown, itemPath: string) => T | null,
): T[] {
  const v = o[key];
  if (v === undefined) {
    if (rustRequired) r.rustMissing.push(`${path}${key}`);
    return [];
  }
  if (!Array.isArray(v)) {
    r.schemaErrors.push(`${path}${key} must be an array`);
    return [];
  }
  const out: T[] = [];
  v.forEach((value, i) => {
    const parsed = item(value, `${path}${key}[${i}].`);
    if (parsed !== null) out.push(parsed);
  });
  return out;
}

const MODEL_KEYS = ['provider', 'model_sku', 'routing_reason', 'policy_digest'] as const;
const EVIDENCE_KEYS = ['evidence_id', 'grade', 'description', 'artifact_digest'] as const;
const FILE_KEYS = ['path', 'preimage_hash', 'postimage_hash'] as const;
const CHANGESET_KEYS = ['files_modified', 'diff_digest'] as const;
const STATEMENT_KEYS = ['action_id', 'statement_digest', 'signature'] as const;
const RECEIPT_KEYS = [
  'schema_version',
  'run_id',
  'session_id',
  'workspace_fingerprint',
  'base_commit_sha',
  'model_selection',
  'event_hash_chain_root',
  'isolation_label',
  'completion_state',
  'evidence_items',
  'changeset',
  'runner_execution_statements',
  'child_receipt_roots',
  'device_signature',
] as const;

const both = { schemaRequired: true, rustRequired: true };
const rustOnly = { schemaRequired: false, rustRequired: true };

export function readReceipt(text: string): ReceiptReading {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    const detail = error instanceof Error ? error.message : 'unreadable';
    return { receipt: null, schemaErrors: [`not JSON: ${detail}`], rustMissing: [], uncovered: [] };
  }
  if (!isObj(parsed)) {
    return { receipt: null, schemaErrors: ['a receipt is a JSON object'], rustMissing: [], uncovered: [] };
  }

  const r: Reader = { schemaErrors: [], rustMissing: [], uncovered: [] };
  const o = parsed;
  noteUncovered(r, o, RECEIPT_KEYS, '');

  const schema_version = readString(r, o, 'schema_version', '', both);
  if (o.schema_version !== undefined && schema_version !== RECEIPT_SCHEMA_VERSION) {
    r.schemaErrors.push(`schema_version must be "${RECEIPT_SCHEMA_VERSION}"`);
  }

  let model_selection: ModelSelection = { provider: '', model_sku: '', routing_reason: '', policy_digest: null };
  if (o.model_selection === undefined) {
    r.schemaErrors.push('model_selection is required');
    r.rustMissing.push('model_selection');
  } else if (!isObj(o.model_selection)) {
    r.schemaErrors.push('model_selection must be an object');
  } else {
    const m = o.model_selection;
    noteUncovered(r, m, MODEL_KEYS, 'model_selection.');
    model_selection = {
      provider: readString(r, m, 'provider', 'model_selection.', both),
      model_sku: readString(r, m, 'model_sku', 'model_selection.', both),
      routing_reason: readString(r, m, 'routing_reason', 'model_selection.', both),
      policy_digest: readOptionalString(r, m, 'policy_digest', 'model_selection.'),
    };
  }

  const isolation_label = readString(r, o, 'isolation_label', '', both);
  if (o.isolation_label !== undefined && !(ISOLATION_LABELS as readonly string[]).includes(isolation_label)) {
    r.schemaErrors.push(`isolation_label "${isolation_label}" is not one of ${ISOLATION_LABELS.join(', ')}`);
  }
  const completion_state = readString(r, o, 'completion_state', '', both);
  if (o.completion_state !== undefined && !(COMPLETION_STATES as readonly string[]).includes(completion_state)) {
    r.schemaErrors.push(`completion_state "${completion_state}" is not one of ${COMPLETION_STATES.join(', ')}`);
  }

  const evidence_items = readList(r, o, 'evidence_items', '', true, (value, path) => {
    if (!isObj(value)) {
      r.schemaErrors.push(`${path.slice(0, -1)} must be an object`);
      return null;
    }
    noteUncovered(r, value, EVIDENCE_KEYS, path);
    const grade = readString(r, value, 'grade', path, both);
    if (value.grade !== undefined && !(EVIDENCE_GRADES as readonly string[]).includes(grade)) {
      r.schemaErrors.push(`${path}grade "${grade}" is not a known grade`);
    }
    return {
      evidence_id: readString(r, value, 'evidence_id', path, both),
      grade,
      description: readString(r, value, 'description', path, both),
      artifact_digest: readOptionalString(r, value, 'artifact_digest', path),
    };
  });

  let changeset: ChangeSet = { files_modified: [], diff_digest: '' };
  if (o.changeset === undefined) {
    r.rustMissing.push('changeset');
  } else if (!isObj(o.changeset)) {
    r.schemaErrors.push('changeset must be an object');
  } else {
    const c = o.changeset;
    noteUncovered(r, c, CHANGESET_KEYS, 'changeset.');
    changeset = {
      files_modified: readList(r, c, 'files_modified', 'changeset.', true, (value, path) => {
        if (!isObj(value)) {
          r.schemaErrors.push(`${path.slice(0, -1)} must be an object`);
          return null;
        }
        noteUncovered(r, value, FILE_KEYS, path);
        return {
          path: readString(r, value, 'path', path, both),
          preimage_hash: readString(r, value, 'preimage_hash', path, both),
          postimage_hash: readString(r, value, 'postimage_hash', path, both),
        };
      }),
      diff_digest: readString(r, c, 'diff_digest', 'changeset.', rustOnly),
    };
  }

  const runner_execution_statements = readList(r, o, 'runner_execution_statements', '', true, (value, path) => {
    if (!isObj(value)) {
      r.schemaErrors.push(`${path.slice(0, -1)} must be an object`);
      return null;
    }
    noteUncovered(r, value, STATEMENT_KEYS, path);
    return {
      action_id: readString(r, value, 'action_id', path, both),
      statement_digest: readString(r, value, 'statement_digest', path, both),
      signature: readString(r, value, 'signature', path, both),
    };
  });

  const child_receipt_roots = readList(r, o, 'child_receipt_roots', '', false, (value, path) => {
    if (typeof value !== 'string') {
      r.schemaErrors.push(`${path.slice(0, -1)} must be a string`);
      return null;
    }
    return value;
  });

  const receipt: Receipt = {
    schema_version,
    run_id: readString(r, o, 'run_id', '', both),
    session_id: readString(r, o, 'session_id', '', both),
    workspace_fingerprint: readString(r, o, 'workspace_fingerprint', '', both),
    base_commit_sha: readString(r, o, 'base_commit_sha', '', both),
    model_selection,
    event_hash_chain_root: readString(r, o, 'event_hash_chain_root', '', both),
    isolation_label,
    completion_state,
    evidence_items,
    changeset,
    runner_execution_statements,
    child_receipt_roots,
    // The schema requires it; the struct defaults it to empty.
    device_signature: readString(r, o, 'device_signature', '', { schemaRequired: true, rustRequired: false }),
  };

  return { receipt, schemaErrors: r.schemaErrors, rustMissing: r.rustMissing, uncovered: r.uncovered };
}

// ---- canonical bytes ---------------------------------------------------------

type Json = string | number | boolean | null | Json[] | { [key: string]: Json };

/** The struct as serde_json::to_value would produce it, signature emptied. */
export function signedValue(receipt: Receipt): Json {
  const model: { [key: string]: Json } = {
    provider: receipt.model_selection.provider,
    model_sku: receipt.model_selection.model_sku,
    routing_reason: receipt.model_selection.routing_reason,
  };
  if (receipt.model_selection.policy_digest !== null) model.policy_digest = receipt.model_selection.policy_digest;

  const value: { [key: string]: Json } = {
    schema_version: receipt.schema_version,
    run_id: receipt.run_id,
    session_id: receipt.session_id,
    workspace_fingerprint: receipt.workspace_fingerprint,
    base_commit_sha: receipt.base_commit_sha,
    model_selection: model,
    event_hash_chain_root: receipt.event_hash_chain_root,
    isolation_label: receipt.isolation_label,
    completion_state: receipt.completion_state,
    evidence_items: receipt.evidence_items.map((item) => {
      const out: { [key: string]: Json } = {
        evidence_id: item.evidence_id,
        grade: item.grade,
        description: item.description,
      };
      if (item.artifact_digest !== null) out.artifact_digest = item.artifact_digest;
      return out;
    }),
    changeset: {
      files_modified: receipt.changeset.files_modified.map((f) => ({
        path: f.path,
        preimage_hash: f.preimage_hash,
        postimage_hash: f.postimage_hash,
      })),
      diff_digest: receipt.changeset.diff_digest,
    },
    runner_execution_statements: receipt.runner_execution_statements.map((s) => ({
      action_id: s.action_id,
      statement_digest: s.statement_digest,
      signature: s.signature,
    })),
    device_signature: '',
  };
  if (receipt.child_receipt_roots.length > 0) value.child_receipt_roots = [...receipt.child_receipt_roots];
  return value;
}

/**
 * VitnaRunReceiptV1::canonicalize_value, line for line. JSON.stringify escapes
 * a well-formed string exactly as serde_json does (the short escapes, other
 * control characters as lowercase \u00xx, nothing else), and every key here
 * is ASCII, so sorting UTF-16 code units matches Rust's byte order.
 */
export function canonicalJson(value: Json): string {
  if (value === null) return 'null';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number') return JSON.stringify(value);
  if (typeof value === 'string') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(value[k] as Json)}`).join(',')}}`;
}

export function canonicalBytes(receipt: Receipt): Uint8Array {
  return new TextEncoder().encode(canonicalJson(signedValue(receipt)));
}
