import { describe, expect, it } from 'vitest';
import {
  canonicalPolicyBytes,
  checkModelPolicyAgainstReceipt,
  MODEL_POLICY_SCHEMA_V1,
  sandboxSatisfiesRequirement,
  type SignedModelPolicy,
} from './policy';
import type { Receipt } from './receipt';
import { bytesToHex } from './verify';

async function makeSignedPolicy(overrides: Partial<SignedModelPolicy> = {}): Promise<{
  policy: SignedModelPolicy;
  json: string;
  digest: string;
}> {
  const pair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  const pubRaw = new Uint8Array(await crypto.subtle.exportKey('raw', pair.publicKey));
  const policy: SignedModelPolicy = {
    schema_version: MODEL_POLICY_SCHEMA_V1,
    policy_id: 'desktop-test-policy-v1',
    issued_at: 1000,
    expires_at: 5000,
    allowed_providers: ['vitna-runtime'],
    lanes: [{ lane: 'vitna/code', provider: 'vitna-runtime', model_sku: 'vitna/code' }],
    required_sandbox_guarantee: 'guarded',
    public_key: bytesToHex(pubRaw),
    signature: '',
    ...overrides,
  };
  const canonical = canonicalPolicyBytes(policy);
  const sigBuf = await crypto.subtle.sign({ name: 'Ed25519' }, pair.privateKey, canonical);
  policy.signature = bytesToHex(new Uint8Array(sigBuf));
  const digestBuf = await crypto.subtle.digest('SHA-256', canonical);
  const digest = bytesToHex(new Uint8Array(digestBuf));
  return { policy, json: JSON.stringify(policy, null, 2), digest };
}

function makeReceipt(policyDigest: string | null, overrides: Partial<Receipt> = {}): Receipt {
  return {
    schema_version: 'vitna-run-receipt-v1',
    run_id: 'run-policy-desktop-1',
    session_id: 'sess-1',
    workspace_fingerprint: 'a'.repeat(64),
    base_commit_sha: 'b'.repeat(40),
    model_selection: {
      provider: 'vitna-runtime',
      model_sku: 'vitna/code',
      routing_reason: 'runtime_lane:vitna/code',
      policy_digest: policyDigest,
    },
    event_hash_chain_root: 'c'.repeat(64),
    isolation_label: 'guarded',
    completion_state: 'completed_with_evidence',
    evidence_items: [],
    changeset: { files_modified: [], diff_digest: 'd'.repeat(64) },
    runner_execution_statements: [],
    egress_calls: [],
    child_receipt_roots: [],
    device_public_key: null,
    device_signature: '',
    ...overrides,
  };
}

describe('checkModelPolicyAgainstReceipt', () => {
  it('matches a valid signed policy whose digest, route, and sandbox align with the receipt', async () => {
    const { json, digest } = await makeSignedPolicy();
    const receipt = makeReceipt(digest);
    const res = await checkModelPolicyAgainstReceipt(json, receipt, 2000);
    expect(res).toEqual({
      status: 'matched',
      policyId: 'desktop-test-policy-v1',
      policyDigest: digest,
      requiredSandbox: 'guarded',
    });
  });

  it('rejects a tampered policy signature fail-closed', async () => {
    const { json, digest } = await makeSignedPolicy();
    const tampered = json.replace('"guarded"', '"full_access"');
    const receipt = makeReceipt(digest);
    const res = await checkModelPolicyAgainstReceipt(tampered, receipt, 2000);
    expect(res.status).toBe('invalid');
    if (res.status === 'invalid') {
      expect(res.errors[0]).toContain('signature is invalid');
    }
  });

  it('rejects an expired signed policy', async () => {
    const { json, digest } = await makeSignedPolicy({ expires_at: 1500 });
    const receipt = makeReceipt(digest);
    const res = await checkModelPolicyAgainstReceipt(json, receipt, 2000);
    expect(res.status).toBe('invalid');
    if (res.status === 'invalid') {
      expect(res.errors[0]).toContain('expired');
    }
  });

  it('flags provider, sandbox downgrade, and policy_digest mismatches', async () => {
    const { json } = await makeSignedPolicy({ required_sandbox_guarantee: 'strong' });
    const receipt = makeReceipt('0'.repeat(64), {
      isolation_label: 'guarded',
      model_selection: {
        provider: 'openai',
        model_sku: 'gpt-5',
        routing_reason: 'direct',
        policy_digest: '0'.repeat(64),
      },
    });
    const res = await checkModelPolicyAgainstReceipt(json, receipt, 2000);
    expect(res.status).toBe('mismatch');
    if (res.status === 'mismatch') {
      expect(res.errors.length).toBeGreaterThanOrEqual(3);
    }
  });

  it('enforces sandbox guarantee hierarchy', () => {
    expect(sandboxSatisfiesRequirement('strong', 'guarded')).toBe(true);
    expect(sandboxSatisfiesRequirement('guarded', 'guarded')).toBe(true);
    expect(sandboxSatisfiesRequirement('full_access', 'guarded')).toBe(false);
    expect(sandboxSatisfiesRequirement('guarded', 'strong')).toBe(false);
    expect(sandboxSatisfiesRequirement('strong', 'strong')).toBe(true);
  });
});
