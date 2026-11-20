/**
 * In-browser verification of Ed25519-signed `vitna-model-policy-v1` files
 * against a loaded `vitna-run-receipt-v1`, matching `crates/policy/src/lib.rs`
 * and `vitna verify --policy` field for field using WebCrypto only.
 */

import type { Receipt } from './receipt';
import { bytesToHex, hexToBytes } from './verify';

export const MODEL_POLICY_SCHEMA_V1 = 'vitna-model-policy-v1';
export const VALID_SANDBOX_GUARANTEES = ['read_only', 'guarded', 'strong', 'full_access'] as const;

export interface ModelLaneConfig {
  lane: string;
  provider: string;
  model_sku: string;
}

export interface SignedModelPolicy {
  schema_version: string;
  policy_id: string;
  issued_at: number;
  expires_at: number;
  allowed_providers: string[];
  lanes: ModelLaneConfig[];
  required_sandbox_guarantee: string;
  public_key: string;
  signature: string;
}

export type PolicyCheck =
  | {
      status: 'matched';
      policyId: string;
      policyDigest: string;
      requiredSandbox: string;
    }
  | {
      status: 'mismatch';
      policyId: string;
      policyDigest: string;
      errors: string[];
    }
  | {
      status: 'invalid';
      policyId: string | null;
      errors: string[];
    }
  | {
      status: 'unsupported';
      detail: string;
    };

function canonicalizeValue(val: unknown): string {
  if (val === null) return 'null';
  if (typeof val === 'boolean') return val ? 'true' : 'false';
  if (typeof val === 'number') return String(val);
  if (typeof val === 'string') return JSON.stringify(val);
  if (Array.isArray(val)) {
    return `[${val.map(canonicalizeValue).join(',')}]`;
  }
  if (typeof val === 'object') {
    const obj = val as Record<string, unknown>;
    const keys = Object.keys(obj).sort();
    const entries = keys.map((k) => `${JSON.stringify(k)}:${canonicalizeValue(obj[k])}`);
    return `{${entries.join(',')}}`;
  }
  return 'null';
}

/**
 * Rebuilds the exact canonical RFC 8785 byte representation signed by
 * `SignedModelPolicy::to_canonical_bytes` in `crates/policy/src/lib.rs`.
 */
export function canonicalPolicyBytes(policy: SignedModelPolicy): Uint8Array {
  const payload = {
    schema_version: policy.schema_version,
    policy_id: policy.policy_id,
    issued_at: policy.issued_at,
    expires_at: policy.expires_at,
    allowed_providers: policy.allowed_providers,
    lanes: policy.lanes.map((l) => ({
      lane: l.lane,
      provider: l.provider,
      model_sku: l.model_sku,
    })),
    required_sandbox_guarantee: policy.required_sandbox_guarantee,
    public_key: policy.public_key,
    signature: '',
  };
  return new TextEncoder().encode(canonicalizeValue(payload));
}

export function sandboxSatisfiesRequirement(actual: string, required: string): boolean {
  const valid = VALID_SANDBOX_GUARANTEES as readonly string[];
  if (!valid.includes(actual) || !valid.includes(required)) return false;
  switch (required) {
    case 'full_access':
      return true;
    case 'guarded':
      return actual === 'guarded' || actual === 'read_only' || actual === 'strong';
    case 'read_only':
      return actual === 'read_only';
    case 'strong':
      return actual === 'strong';
    default:
      return false;
  }
}

export function parseModelPolicy(rawText: string): { policy: SignedModelPolicy | null; errors: string[] } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(rawText);
  } catch (err) {
    return { policy: null, errors: [`Invalid JSON: ${err instanceof Error ? err.message : String(err)}`] };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { policy: null, errors: ['Model policy must be a JSON object.'] };
  }
  const obj = parsed as Record<string, unknown>;
  const errors: string[] = [];

  if (obj.schema_version !== MODEL_POLICY_SCHEMA_V1) {
    errors.push(`Unsupported schema_version '${String(obj.schema_version ?? '')}': expected '${MODEL_POLICY_SCHEMA_V1}'.`);
  }
  if (typeof obj.policy_id !== 'string' || obj.policy_id.trim() === '') {
    errors.push('Missing non-empty policy_id.');
  }
  if (typeof obj.issued_at !== 'number' || !Number.isFinite(obj.issued_at)) {
    errors.push('Missing numeric issued_at.');
  }
  if (typeof obj.expires_at !== 'number' || !Number.isFinite(obj.expires_at)) {
    errors.push('Missing numeric expires_at.');
  }
  if (!Array.isArray(obj.allowed_providers) || !obj.allowed_providers.every((p) => typeof p === 'string')) {
    errors.push('allowed_providers must be an array of strings.');
  }
  if (
    !Array.isArray(obj.lanes) ||
    !obj.lanes.every(
      (l) =>
        l &&
        typeof l === 'object' &&
        typeof (l as Record<string, unknown>).lane === 'string' &&
        typeof (l as Record<string, unknown>).provider === 'string' &&
        typeof (l as Record<string, unknown>).model_sku === 'string',
    )
  ) {
    errors.push('lanes must be an array of { lane, provider, model_sku } objects.');
  }
  if (
    typeof obj.required_sandbox_guarantee !== 'string' ||
    !(VALID_SANDBOX_GUARANTEES as readonly string[]).includes(obj.required_sandbox_guarantee)
  ) {
    errors.push(
      `Invalid required_sandbox_guarantee '${String(obj.required_sandbox_guarantee ?? '')}': expected one of ${VALID_SANDBOX_GUARANTEES.join(', ')}.`,
    );
  }
  if (typeof obj.public_key !== 'string' || obj.public_key.trim() === '') {
    errors.push('Missing public_key.');
  }
  if (typeof obj.signature !== 'string' || obj.signature.trim() === '') {
    errors.push('Missing signature.');
  }

  if (errors.length > 0) {
    return { policy: null, errors };
  }

  return {
    policy: {
      schema_version: obj.schema_version as string,
      policy_id: obj.policy_id as string,
      issued_at: obj.issued_at as number,
      expires_at: obj.expires_at as number,
      allowed_providers: obj.allowed_providers as string[],
      lanes: (obj.lanes as ModelLaneConfig[]).map((l) => ({
        lane: l.lane,
        provider: l.provider,
        model_sku: l.model_sku,
      })),
      required_sandbox_guarantee: obj.required_sandbox_guarantee as string,
      public_key: obj.public_key as string,
      signature: obj.signature as string,
    },
    errors: [],
  };
}

/**
 * Verifies a signed `vitna-model-policy-v1` JSON string via WebCrypto Ed25519
 * + SHA-256 and checks that `receipt` satisfies its providers, lanes, sandbox
 * floor, and canonical `policy_digest`.
 */
export async function checkModelPolicyAgainstReceipt(
  rawPolicyText: string,
  receipt: Receipt,
  nowUnixSecs: number = Math.floor(Date.now() / 1000),
): Promise<PolicyCheck> {
  const { policy, errors: parseErrors } = parseModelPolicy(rawPolicyText);
  if (!policy) {
    return { status: 'invalid', policyId: null, errors: parseErrors };
  }

  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    return { status: 'unsupported', detail: 'This window has no WebCrypto.' };
  }

  const pubBytes = hexToBytes(policy.public_key);
  if (!pubBytes || pubBytes.length !== 32) {
    return {
      status: 'invalid',
      policyId: policy.policy_id,
      errors: ['Policy public_key must be 64 hex characters (32-byte Ed25519 key).'],
    };
  }
  const sigBytes = hexToBytes(policy.signature);
  if (!sigBytes || sigBytes.length !== 64) {
    return {
      status: 'invalid',
      policyId: policy.policy_id,
      errors: ['Policy signature must be 128 hex characters (64-byte Ed25519 signature).'],
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
      policyId: policy.policy_id,
      errors: [err instanceof Error ? err.message : 'Policy public key could not be imported.'],
    };
  }

  const canonical = canonicalPolicyBytes(policy);
  const sigOk = await subtle.verify({ name: 'Ed25519' }, imported, sigBytes, canonical);
  if (!sigOk) {
    return {
      status: 'invalid',
      policyId: policy.policy_id,
      errors: ['Model policy Ed25519 signature is invalid or payload was tampered.'],
    };
  }

  if (nowUnixSecs > policy.expires_at) {
    return {
      status: 'invalid',
      policyId: policy.policy_id,
      errors: [`Model policy '${policy.policy_id}' expired at ${policy.expires_at}.`],
    };
  }

  const digestBuf = await subtle.digest('SHA-256', canonical);
  const policyDigest = bytesToHex(new Uint8Array(digestBuf));

  const complianceErrors: string[] = [];
  const provider = receipt.model_selection.provider;
  const modelSku = receipt.model_selection.model_sku;

  if (
    policy.allowed_providers.length > 0 &&
    !policy.allowed_providers.some((p) => p.toLowerCase() === provider.toLowerCase())
  ) {
    complianceErrors.push(
      `Provider '${provider}' is not permitted by policy '${policy.policy_id}' (allowed: ${policy.allowed_providers.join(', ')}).`,
    );
  }

  if (policy.lanes.length > 0) {
    const laneOk = policy.lanes.some(
      (l) => l.provider.toLowerCase() === provider.toLowerCase() && (l.model_sku === modelSku || l.lane === modelSku),
    );
    if (!laneOk) {
      complianceErrors.push(`Model '${modelSku}' on provider '${provider}' is not in any lane allowed by policy '${policy.policy_id}'.`);
    }
  }

  if (!sandboxSatisfiesRequirement(receipt.isolation_label, policy.required_sandbox_guarantee)) {
    complianceErrors.push(
      `Receipt isolation '${receipt.isolation_label}' does not satisfy policy requirement '${policy.required_sandbox_guarantee}'.`,
    );
  }

  const stampedDigest = receipt.model_selection.policy_digest?.trim() ?? '';
  if (!stampedDigest) {
    complianceErrors.push('Receipt is missing model_selection.policy_digest.');
  } else if (stampedDigest.toLowerCase() !== policyDigest.toLowerCase()) {
    complianceErrors.push(
      `Receipt policy_digest (${stampedDigest.slice(0, 12)}) does not match policy digest (${policyDigest.slice(0, 12)}).`,
    );
  }

  if (complianceErrors.length > 0) {
    return {
      status: 'mismatch',
      policyId: policy.policy_id,
      policyDigest,
      errors: complianceErrors,
    };
  }

  return {
    status: 'matched',
    policyId: policy.policy_id,
    policyDigest,
    requiredSandbox: policy.required_sandbox_guarantee,
  };
}
