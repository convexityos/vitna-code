/**
 * In-browser verification of `vitna-mandate-config-v1` (`am/1` Agent Mandate)
 * bundles against a loaded `vitna-run-receipt-v1`, matching
 * `crates/policy/src/mandate.rs` and `vitna verify --mandate` using WebCrypto
 * Ed25519 + SHA-256 only (zero network, zero external dependencies).
 */

import type { Receipt } from './receipt';
import { bytesToHex } from './verify';

export const MANDATE_CONFIG_SCHEMA_V1 = 'vitna-mandate-config-v1';
export const MANDATE_PROTOCOL_VERSION = 'am/1';
export const MANDATE_JWS_TYP = 'application/agent-mandate+jwt';
export const MANDATE_MAX_DEPTH = 4;
export const MANDATE_DEFAULT_SKEW_SECONDS = 60;

const SCALE_FACTOR = 1_000_000_000n;
const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const BASE58_MAP = new Map<string, number>(
  Array.from(BASE58_ALPHABET).map((ch, idx) => [ch, idx]),
);

const REGISTERED_TOP_CLAIMS = new Set([
  'v',
  'typ',
  'jti',
  'iss',
  'sub',
  'par',
  'iat',
  'nbf',
  'exp',
  'cnf',
  'ledger',
  'cav',
  'st',
]);

const REGISTERED_CAVEATS = new Set([
  'max_total',
  'max_per_txn',
  'rate',
  'aud',
  'resources',
  'purpose',
  'task',
  'payer',
  'depth',
  'delegable',
  'currency',
  'settle',
  'step_up',
  'effect',
]);

export interface Money {
  cur: string;
  amt: string;
}

export interface MandateBundleConfig {
  schema_version: string;
  tokens: string[];
  trusted_root_issuers: string[];
  ledger: string;
  currency: string;
}

export type MandateCheck =
  | {
      status: 'matched';
      rootJti: string;
      chainHash: string;
      path: string[];
      totalSpent: string;
      maxTotal: string | null;
    }
  | {
      status: 'mismatch';
      rootJti: string;
      chainHash: string;
      errors: string[];
    }
  | {
      status: 'invalid';
      rootJti: string | null;
      errors: string[];
    }
  | {
      status: 'unsupported';
      detail: string;
    };

export function encodeBase58(bytes: Uint8Array): string {
  if (bytes.length === 0) return '';
  let leadingZeros = 0;
  while (leadingZeros < bytes.length && bytes[leadingZeros] === 0) {
    leadingZeros += 1;
  }
  let value = 0n;
  for (const b of bytes) {
    value = (value << 8n) + BigInt(b);
  }
  const chars: string[] = [];
  while (value > 0n) {
    const rem = Number(value % 58n);
    chars.push(BASE58_ALPHABET[rem] ?? '1');
    value /= 58n;
  }
  for (let i = 0; i < leadingZeros; i += 1) {
    chars.push('1');
  }
  return chars.reverse().join('');
}

export function decodeBase58(input: string): Uint8Array | null {
  if (input.length === 0) return null;
  let leadingOnes = 0;
  while (leadingOnes < input.length && input[leadingOnes] === '1') {
    leadingOnes += 1;
  }
  let value = 0n;
  for (const ch of input) {
    const digit = BASE58_MAP.get(ch);
    if (digit === undefined) return null;
    value = value * 58n + BigInt(digit);
  }
  const bytes: number[] = [];
  while (value > 0n) {
    bytes.push(Number(value & 0xffn));
    value >>= 8n;
  }
  for (let i = 0; i < leadingOnes; i += 1) {
    bytes.push(0);
  }
  bytes.reverse();
  return new Uint8Array(bytes);
}

/** Encodes a 32-byte Ed25519 public key as `did:key:z<base58btc(0xed01 || pubkey)>`. */
export function publicKeyBytesToDidKey(pubKey: Uint8Array): string {
  const multi = new Uint8Array(34);
  multi[0] = 0xed;
  multi[1] = 0x01;
  multi.set(pubKey, 2);
  return `did:key:z${encodeBase58(multi)}`;
}

/** Decodes `did:key:z...` (with optional `#z...` fragment) into a 32-byte Ed25519 public key. */
export function didKeyToPublicKeyBytes(did: string): Uint8Array | null {
  const hashIdx = did.indexOf('#');
  const bare = hashIdx >= 0 ? did.slice(0, hashIdx) : did;
  const frag = hashIdx >= 0 ? did.slice(hashIdx + 1) : null;
  if (!bare.startsWith('did:key:z')) return null;
  if (frag !== null && frag !== bare.slice('did:key:'.length)) return null;
  const decoded = decodeBase58(bare.slice('did:key:z'.length));
  if (!decoded || decoded.length !== 34 || decoded[0] !== 0xed || decoded[1] !== 0x01) {
    return null;
  }
  return decoded.slice(2);
}

export function encodeBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (const b of bytes) {
    binary += String.fromCharCode(b);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function decodeBase64Url(input: string): Uint8Array | null {
  if (!input || !/^[A-Za-z0-9_-]+$/.test(input)) return null;
  const padded = input.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (input.length % 4)) % 4);
  try {
    const binary = atob(padded);
    const out = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      out[i] = binary.charCodeAt(i);
    }
    return out;
  } catch {
    return null;
  }
}

/** Exact 9-decimal monetary scaling matching `Money::to_scaled` in `crates/policy/src/mandate.rs`. */
export function parseScaledMoney(amt: string, cur: string): bigint | null {
  if (cur.length < 3 || cur.length > 8 || !/^[A-Z]+$/.test(cur)) {
    return null;
  }
  if (!amt) return null;
  const dotIdx = amt.indexOf('.');
  const wholeStr = dotIdx >= 0 ? amt.slice(0, dotIdx) : amt;
  const fracStr = dotIdx >= 0 ? amt.slice(dotIdx + 1) : '';
  if (!wholeStr || !/^\d+$/.test(wholeStr)) return null;
  if (dotIdx >= 0 && (!fracStr || fracStr.length > 9 || !/^\d+$/.test(fracStr))) {
    return null;
  }
  const whole = BigInt(wholeStr);
  const frac = fracStr ? BigInt(fracStr.padEnd(9, '0')) : 0n;
  return whole * SCALE_FACTOR + frac;
}

export function parseMandateBundle(rawText: string): { bundle: MandateBundleConfig | null; errors: string[] } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawText);
  } catch (err) {
    return { bundle: null, errors: [`Invalid JSON: ${err instanceof Error ? err.message : String(err)}`] };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { bundle: null, errors: ['Mandate bundle must be a JSON object.'] };
  }
  const obj = parsed as Record<string, unknown>;
  const errors: string[] = [];
  if (obj.schema_version !== MANDATE_CONFIG_SCHEMA_V1) {
    errors.push(
      `Unsupported schema_version '${String(obj.schema_version ?? '')}': expected '${MANDATE_CONFIG_SCHEMA_V1}'.`,
    );
  }
  if (!Array.isArray(obj.tokens) || obj.tokens.length === 0 || !obj.tokens.every((t) => typeof t === 'string' && t.length > 0)) {
    errors.push('Mandate bundle must contain at least one am/1 JWS token.');
  }
  if (
    !Array.isArray(obj.trusted_root_issuers) ||
    obj.trusted_root_issuers.length === 0 ||
    !obj.trusted_root_issuers.every((iss) => typeof iss === 'string' && iss.trim() !== '')
  ) {
    errors.push('Mandate bundle must specify at least one trusted_root_issuers did:key.');
  }
  if (typeof obj.ledger !== 'string' || obj.ledger.trim() === '') {
    errors.push('Mandate bundle ledger must be non-empty.');
  }
  if (typeof obj.currency !== 'string' || obj.currency.trim() === '') {
    errors.push('Mandate bundle currency must be non-empty.');
  }
  if (errors.length > 0) {
    return { bundle: null, errors };
  }
  return {
    bundle: {
      schema_version: obj.schema_version as string,
      tokens: obj.tokens as string[],
      trusted_root_issuers: obj.trusted_root_issuers as string[],
      ledger: obj.ledger as string,
      currency: obj.currency as string,
    },
    errors: [],
  };
}

interface ParsedMandateLink {
  jti: string;
  iss: string;
  sub: string;
  par: string | null;
  nbf: number;
  exp: number;
  ledger: string;
  delegable: boolean | null;
  depth: number | null;
  maxTotal: Money | null;
  signingInput: string;
  signature: Uint8Array;
}

function parseMoneyObject(raw: unknown): Money | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
  const obj = raw as Record<string, unknown>;
  if (typeof obj.cur !== 'string' || typeof obj.amt !== 'string') return null;
  if (parseScaledMoney(obj.amt, obj.cur) === null) return null;
  return { cur: obj.cur, amt: obj.amt };
}

function parseMandateJws(token: string): { link: ParsedMandateLink | null; error: string | null } {
  const parts = token.split('.');
  if (parts.length !== 3 || !parts[0] || !parts[1] || !parts[2]) {
    return { link: null, error: 'JWS must have exactly three non-empty segments.' };
  }
  const [hSeg, pSeg, sSeg] = parts;
  const hBytes = decodeBase64Url(hSeg);
  const pBytes = decodeBase64Url(pSeg);
  const sigBytes = decodeBase64Url(sSeg);
  if (!hBytes || !pBytes || !sigBytes) {
    return { link: null, error: 'JWS contains invalid base64url encoding.' };
  }
  if (sigBytes.length !== 64) {
    return { link: null, error: 'Ed25519 signature must be 64 bytes.' };
  }

  let header: unknown;
  let payload: unknown;
  try {
    header = JSON.parse(new TextDecoder().decode(hBytes));
    payload = JSON.parse(new TextDecoder().decode(pBytes));
  } catch {
    return { link: null, error: 'JWS header or payload is not valid JSON.' };
  }

  if (!header || typeof header !== 'object' || Array.isArray(header)) {
    return { link: null, error: 'JWS header must be a JSON object.' };
  }
  const hObj = header as Record<string, unknown>;
  if (hObj.alg !== 'EdDSA') {
    return { link: null, error: 'Unsupported JWS alg; only EdDSA is permitted.' };
  }
  if (hObj.typ !== MANDATE_JWS_TYP) {
    return { link: null, error: `Unexpected JWS typ; expected ${MANDATE_JWS_TYP}.` };
  }

  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    return { link: null, error: 'JWS payload must be a JSON object.' };
  }
  const pObj = payload as Record<string, unknown>;
  for (const key of Object.keys(pObj)) {
    if (!REGISTERED_TOP_CLAIMS.has(key)) {
      return { link: null, error: `Unregistered top-level mandate claim '${key}'.` };
    }
  }
  if (!pObj.cav || typeof pObj.cav !== 'object' || Array.isArray(pObj.cav)) {
    return { link: null, error: 'Mandate payload is missing cav object.' };
  }
  const cavObj = pObj.cav as Record<string, unknown>;
  for (const key of Object.keys(cavObj)) {
    if (!REGISTERED_CAVEATS.has(key)) {
      return { link: null, error: `Unregistered mandate caveat '${key}'.` };
    }
  }

  if (pObj.v !== MANDATE_PROTOCOL_VERSION) {
    return { link: null, error: `Unsupported mandate version '${String(pObj.v ?? '')}'.` };
  }
  if (pObj.typ !== 'mandate') {
    return { link: null, error: `Unexpected mandate typ '${String(pObj.typ ?? '')}'.` };
  }
  if (
    typeof pObj.jti !== 'string' ||
    !pObj.jti ||
    typeof pObj.iss !== 'string' ||
    !pObj.iss ||
    typeof pObj.sub !== 'string' ||
    !pObj.sub ||
    typeof pObj.ledger !== 'string' ||
    !pObj.ledger
  ) {
    return { link: null, error: 'Mandate claims require non-empty jti, iss, sub, and ledger.' };
  }
  if (pObj.par !== undefined && pObj.par !== null && typeof pObj.par !== 'string') {
    return { link: null, error: 'Mandate par claim must be string or null.' };
  }
  if (typeof pObj.nbf !== 'number' || typeof pObj.exp !== 'number' || !Number.isFinite(pObj.nbf) || !Number.isFinite(pObj.exp)) {
    return { link: null, error: 'Mandate claims require numeric nbf and exp.' };
  }
  if (pObj.exp <= pObj.nbf) {
    return { link: null, error: 'Mandate exp must be greater than nbf.' };
  }

  let maxTotal: Money | null = null;
  if (cavObj.max_total !== undefined && cavObj.max_total !== null) {
    maxTotal = parseMoneyObject(cavObj.max_total);
    if (!maxTotal) {
      return { link: null, error: 'Mandate cav.max_total is not a valid Money object.' };
    }
  }

  let delegable: boolean | null = null;
  if (cavObj.delegable !== undefined && cavObj.delegable !== null) {
    if (typeof cavObj.delegable !== 'boolean') {
      return { link: null, error: 'Mandate cav.delegable must be boolean.' };
    }
    delegable = cavObj.delegable;
  }

  let depth: number | null = null;
  if (cavObj.depth !== undefined && cavObj.depth !== null) {
    if (typeof cavObj.depth !== 'number' || !Number.isInteger(cavObj.depth) || cavObj.depth < 0) {
      return { link: null, error: 'Mandate cav.depth must be a non-negative integer.' };
    }
    depth = cavObj.depth;
  }

  return {
    link: {
      jti: pObj.jti,
      iss: pObj.iss,
      sub: pObj.sub,
      par: typeof pObj.par === 'string' ? pObj.par : null,
      nbf: pObj.nbf,
      exp: pObj.exp,
      ledger: pObj.ledger,
      delegable,
      depth,
      maxTotal,
      signingInput: `${hSeg}.${pSeg}`,
      signature: sigBytes,
    },
    error: null,
  };
}

/**
 * Verifies an `am/1` `vitna-mandate-config-v1` bundle's Ed25519 JWS chain via
 * WebCrypto and asserts that `receipt.mandate_provenance` matches `root_jti`,
 * `chain_hash`, delegation `path`, `ledger`, `currency`, and `max_total` cap.
 */
export async function checkMandateBundleAgainstReceipt(
  rawBundleText: string,
  receipt: Receipt,
  nowUnixSecs: number = Math.floor(Date.now() / 1000),
): Promise<MandateCheck> {
  const { bundle, errors: parseErrors } = parseMandateBundle(rawBundleText);
  if (!bundle) {
    return { status: 'invalid', rootJti: null, errors: parseErrors };
  }

  if (bundle.tokens.length - 1 > MANDATE_MAX_DEPTH) {
    return {
      status: 'invalid',
      rootJti: null,
      errors: [`Chain depth ${bundle.tokens.length - 1} exceeds am/1 maximum (${MANDATE_MAX_DEPTH}).`],
    };
  }

  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    return { status: 'unsupported', detail: 'This window has no WebCrypto.' };
  }

  const links: ParsedMandateLink[] = [];
  for (let i = 0; i < bundle.tokens.length; i += 1) {
    const token = bundle.tokens[i];
    if (!token) {
      return { status: 'invalid', rootJti: null, errors: [`Link ${i} is empty.`] };
    }
    const { link, error } = parseMandateJws(token);
    if (!link) {
      return { status: 'invalid', rootJti: links[0]?.jti ?? null, errors: [`Link ${i}: ${error ?? 'invalid JWS'}`] };
    }
    links.push(link);
  }

  const root = links[0]!;
  if (root.par !== null) {
    return { status: 'invalid', rootJti: root.jti, errors: ['Root mandate must have par = null.'] };
  }
  if (!bundle.trusted_root_issuers.includes(root.iss)) {
    return { status: 'invalid', rootJti: root.jti, errors: [`Untrusted root issuer: ${root.iss}`] };
  }

  let effNbf = 0;
  let effExp = Number.MAX_SAFE_INTEGER;
  let effDelegable: boolean | null = null;
  let prevDepth: number | null = null;
  let effMaxTotal: Money | null = null;
  const path: string[] = [];

  for (let idx = 0; idx < links.length; idx += 1) {
    const cur = links[idx]!;
    if (cur.ledger !== bundle.ledger) {
      return {
        status: 'invalid',
        rootJti: root.jti,
        errors: [`Link ${idx} ledger '${cur.ledger}' does not match bundle ledger '${bundle.ledger}'.`],
      };
    }

    if (idx > 0) {
      const parent = links[idx - 1]!;
      if (cur.par !== parent.jti) {
        return { status: 'invalid', rootJti: root.jti, errors: [`Link ${idx} par does not match parent jti.`] };
      }
      if (cur.iss !== parent.sub) {
        return {
          status: 'invalid',
          rootJti: root.jti,
          errors: [`Link ${idx} iss '${cur.iss}' does not match parent sub '${parent.sub}'.`],
        };
      }
      if (effDelegable === false) {
        return {
          status: 'invalid',
          rootJti: root.jti,
          errors: ['Cannot delegate past a mandate with delegable = false.'],
        };
      }
      if (prevDepth !== null) {
        if (prevDepth === 0) {
          return { status: 'invalid', rootJti: root.jti, errors: ['Delegation depth exhausted at parent.'] };
        }
        if (cur.depth !== null && cur.depth > prevDepth - 1) {
          return {
            status: 'invalid',
            rootJti: root.jti,
            errors: [`Child depth ${cur.depth} exceeds parent depth - 1 (${prevDepth - 1}).`],
          };
        }
      }
      if (cur.nbf < effNbf || cur.exp > effExp) {
        return { status: 'invalid', rootJti: root.jti, errors: ['Child validity window widens parent window.'] };
      }
      if (cur.maxTotal && effMaxTotal) {
        if (cur.maxTotal.cur.toUpperCase() !== effMaxTotal.cur.toUpperCase()) {
          return { status: 'invalid', rootJti: root.jti, errors: ['Child max_total currency does not match parent currency.'] };
        }
        const curScaled = parseScaledMoney(cur.maxTotal.amt, cur.maxTotal.cur);
        const effScaled = parseScaledMoney(effMaxTotal.amt, effMaxTotal.cur);
        if (curScaled === null || effScaled === null || curScaled > effScaled) {
          return { status: 'invalid', rootJti: root.jti, errors: ['Child max_total widens parent budget cap.'] };
        }
      }
    }

    const pubBytes = didKeyToPublicKeyBytes(cur.iss);
    if (!pubBytes) {
      return {
        status: 'invalid',
        rootJti: root.jti,
        errors: [`Link ${idx} iss is not a valid Ed25519 did:key: ${cur.iss}`],
      };
    }

    let imported: CryptoKey;
    try {
      imported = await subtle.importKey('raw', pubBytes, { name: 'Ed25519' }, false, ['verify']);
    } catch (err) {
      const name = err instanceof DOMException ? err.name : '';
      if (name === 'NotSupportedError') {
        return { status: 'unsupported', detail: 'This browser has no Ed25519 in WebCrypto.' };
      }
      return {
        status: 'invalid',
        rootJti: root.jti,
        errors: [err instanceof Error ? err.message : `Link ${idx} public key could not be imported.`],
      };
    }

    const sigOk = await subtle.verify(
      { name: 'Ed25519' },
      imported,
      cur.signature,
      new TextEncoder().encode(cur.signingInput),
    );
    if (!sigOk) {
      return {
        status: 'invalid',
        rootJti: root.jti,
        errors: [`Link ${idx} Ed25519 JWS signature verification failed.`],
      };
    }

    if (cur.delegable === false) {
      effDelegable = false;
    } else if (effDelegable === null && cur.delegable !== null) {
      effDelegable = cur.delegable;
    }

    if (prevDepth !== null) {
      const dec = Math.max(0, prevDepth - 1);
      prevDepth = cur.depth !== null ? Math.min(cur.depth, dec) : dec;
    } else if (cur.depth !== null) {
      prevDepth = cur.depth;
    }

    if (cur.maxTotal) {
      if (!effMaxTotal) {
        effMaxTotal = cur.maxTotal;
      } else {
        const curScaled = parseScaledMoney(cur.maxTotal.amt, cur.maxTotal.cur)!;
        const effScaled = parseScaledMoney(effMaxTotal.amt, effMaxTotal.cur)!;
        if (curScaled < effScaled) {
          effMaxTotal = cur.maxTotal;
        }
      }
    }

    effNbf = Math.max(effNbf, cur.nbf);
    effExp = Math.min(effExp, cur.exp);
    path.push(cur.jti);
  }

  if (nowUnixSecs + MANDATE_DEFAULT_SKEW_SECONDS < effNbf || nowUnixSecs > effExp + MANDATE_DEFAULT_SKEW_SECONDS) {
    return {
      status: 'invalid',
      rootJti: root.jti,
      errors: [`Mandate chain is expired or not yet active (window ${effNbf}..${effExp}).`],
    };
  }

  const hashBuf = await subtle.digest('SHA-256', new TextEncoder().encode(bundle.tokens.join('\n')));
  const chainHash = `sha256:${bytesToHex(new Uint8Array(hashBuf))}`;

  const mp = receipt.mandate_provenance;
  if (!mp) {
    return {
      status: 'mismatch',
      rootJti: root.jti,
      chainHash,
      errors: ['Receipt is missing mandate_provenance.'],
    };
  }

  const errors: string[] = [];
  if (mp.root_jti !== root.jti) {
    errors.push(`Receipt mandate root_jti '${mp.root_jti}' does not match verified root '${root.jti}'.`);
  }
  if (mp.chain_hash !== chainHash) {
    errors.push('Receipt mandate chain_hash does not match verified mandate chain.');
  }
  if (mp.path.length !== path.length || mp.path.some((id, i) => id !== path[i])) {
    errors.push(`Receipt delegation path (${mp.path.join(' -> ')}) does not match verified path (${path.join(' -> ')}).`);
  }
  if (mp.ledger !== bundle.ledger) {
    errors.push(`Receipt mandate ledger '${mp.ledger}' does not match bundle ledger '${bundle.ledger}'.`);
  }
  if (mp.currency.toUpperCase() !== bundle.currency.toUpperCase()) {
    errors.push(`Receipt mandate currency '${mp.currency}' does not match bundle currency '${bundle.currency}'.`);
  }

  const spentScaled = parseScaledMoney(mp.total_spent, mp.currency.toUpperCase());
  if (spentScaled === null) {
    errors.push(`Receipt mandate total_spent '${mp.total_spent} ${mp.currency}' is invalid.`);
  } else if (effMaxTotal) {
    if (effMaxTotal.cur.toUpperCase() !== mp.currency.toUpperCase()) {
      errors.push(`Receipt currency '${mp.currency}' does not match max_total currency '${effMaxTotal.cur}'.`);
    } else {
      const capScaled = parseScaledMoney(effMaxTotal.amt, effMaxTotal.cur.toUpperCase());
      if (capScaled === null) {
        errors.push('Verified mandate max_total is invalid.');
      } else if (spentScaled > capScaled) {
        errors.push(
          `Receipt total_spent ${mp.total_spent} ${mp.currency} exceeds mandate max_total ${effMaxTotal.amt} ${effMaxTotal.cur}.`,
        );
      }
    }
  }

  if (errors.length > 0) {
    return { status: 'mismatch', rootJti: root.jti, chainHash, errors };
  }

  return {
    status: 'matched',
    rootJti: root.jti,
    chainHash,
    path,
    totalSpent: `${mp.total_spent} ${mp.currency}`,
    maxTotal: effMaxTotal ? `${effMaxTotal.amt} ${effMaxTotal.cur}` : null,
  };
}
