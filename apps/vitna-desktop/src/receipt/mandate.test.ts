import { describe, expect, it } from 'vitest';
import {
  checkMandateBundleAgainstReceipt,
  didKeyToPublicKeyBytes,
  encodeBase64Url,
  MANDATE_CONFIG_SCHEMA_V1,
  MANDATE_JWS_TYP,
  MANDATE_PROTOCOL_VERSION,
  parseScaledMoney,
  publicKeyBytesToDidKey,
} from './mandate';
import type { MandateProvenance, Receipt } from './receipt';
import { bytesToHex } from './verify';

async function signJws(claims: Record<string, unknown>, privateKey: CryptoKey, kid: string): Promise<string> {
  const header = { alg: 'EdDSA', typ: MANDATE_JWS_TYP, kid };
  const hSeg = encodeBase64Url(new TextEncoder().encode(JSON.stringify(header)));
  const pSeg = encodeBase64Url(new TextEncoder().encode(JSON.stringify(claims)));
  const input = `${hSeg}.${pSeg}`;
  const sig = await crypto.subtle.sign({ name: 'Ed25519' }, privateKey, new TextEncoder().encode(input));
  return `${input}.${encodeBase64Url(new Uint8Array(sig))}`;
}

async function makeSignedMandateBundle(opts?: {
  maxTotal?: string;
  exp?: number;
  withChild?: boolean;
  childMaxTotal?: string;
}): Promise<{
  json: string;
  rootJti: string;
  path: string[];
  chainHash: string;
  ledger: string;
}> {
  const rootPair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  const rootPub = new Uint8Array(await crypto.subtle.exportKey('raw', rootPair.publicKey));
  const rootDid = publicKeyBytesToDidKey(rootPub);

  const agentPair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  const agentPub = new Uint8Array(await crypto.subtle.exportKey('raw', agentPair.publicKey));
  const agentDid = publicKeyBytesToDidKey(agentPub);

  const ledger = 'https://ledger.vitna.local';
  const rootJti = 'mnd-root-desktop-1';
  const exp = opts?.exp ?? 5000;

  const rootToken = await signJws(
    {
      v: MANDATE_PROTOCOL_VERSION,
      typ: 'mandate',
      jti: rootJti,
      iss: rootDid,
      sub: agentDid,
      par: null,
      iat: 1000,
      nbf: 1000,
      exp,
      ledger,
      cav: {
        max_total: { cur: 'USD', amt: opts?.maxTotal ?? '2.00' },
        depth: 2,
        delegable: true,
      },
    },
    rootPair.privateKey,
    `${rootDid}#${rootDid.slice('did:key:'.length)}`,
  );

  const tokens = [rootToken];
  const path = [rootJti];

  if (opts?.withChild) {
    const workerPair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
    const workerPub = new Uint8Array(await crypto.subtle.exportKey('raw', workerPair.publicKey));
    const workerDid = publicKeyBytesToDidKey(workerPub);
    const childJti = 'mnd-child-desktop-2';
    const childToken = await signJws(
      {
        v: MANDATE_PROTOCOL_VERSION,
        typ: 'mandate',
        jti: childJti,
        iss: agentDid,
        sub: workerDid,
        par: rootJti,
        iat: 1100,
        nbf: 1100,
        exp: exp - 100,
        ledger,
        cav: {
          max_total: { cur: 'USD', amt: opts.childMaxTotal ?? '1.00' },
          depth: 1,
          delegable: false,
        },
      },
      agentPair.privateKey,
      `${agentDid}#${agentDid.slice('did:key:'.length)}`,
    );
    tokens.push(childToken);
    path.push(childJti);
  }

  const hashBuf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(tokens.join('\n')));
  const chainHash = `sha256:${bytesToHex(new Uint8Array(hashBuf))}`;
  const bundle = {
    schema_version: MANDATE_CONFIG_SCHEMA_V1,
    tokens,
    trusted_root_issuers: [rootDid],
    ledger,
    currency: 'USD',
  };
  return { json: JSON.stringify(bundle, null, 2), rootJti, path, chainHash, ledger };
}

function makeReceipt(mandate: MandateProvenance | null): Receipt {
  return {
    schema_version: 'vitna-run-receipt-v1',
    run_id: 'run-mandate-desktop-1',
    session_id: 'sess-1',
    workspace_fingerprint: 'a'.repeat(64),
    base_commit_sha: 'b'.repeat(40),
    model_selection: {
      provider: 'vitna-runtime',
      model_sku: 'vitna/code',
      routing_reason: 'runtime_lane:vitna/code',
      policy_digest: null,
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
    mandate_provenance: mandate,
    device_signature: '',
  };
}

describe('checkMandateBundleAgainstReceipt', () => {
  it('round-trips Ed25519 did:key encoding and exact 9-decimal Money scaling', async () => {
    const pair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
    const raw = new Uint8Array(await crypto.subtle.exportKey('raw', pair.publicKey));
    const did = publicKeyBytesToDidKey(raw);
    expect(did.startsWith('did:key:z')).toBe(true);
    expect(didKeyToPublicKeyBytes(did)).toEqual(raw);
    expect(didKeyToPublicKeyBytes(`${did}#${did.slice('did:key:'.length)}`)).toEqual(raw);
    expect(didKeyToPublicKeyBytes(`${did}#wrong`)).toBeNull();
    expect(parseScaledMoney('1.25', 'USD')).toBe(1_250_000_000n);
    expect(parseScaledMoney('0.000000001', 'USD')).toBe(1n);
    expect(parseScaledMoney('1.1234567890', 'USD')).toBeNull();
  });

  it('verifies a 2-hop am/1 delegation chain and matches receipt provenance within budget', async () => {
    const { json, rootJti, path, chainHash, ledger } = await makeSignedMandateBundle({
      maxTotal: '2.00',
      withChild: true,
      childMaxTotal: '1.00',
    });
    const receipt = makeReceipt({
      chain_hash: chainHash,
      root_jti: rootJti,
      path,
      ledger,
      currency: 'USD',
      total_spent: '0.75',
      spend_digests: ['e'.repeat(64)],
    });
    const res = await checkMandateBundleAgainstReceipt(json, receipt, 2000);
    expect(res).toEqual({
      status: 'matched',
      rootJti,
      chainHash,
      path,
      totalSpent: '0.75 USD',
      maxTotal: '1.00 USD',
    });
  });

  it('rejects a receipt whose spend exceeds the effective child max_total cap', async () => {
    const { json, rootJti, path, chainHash, ledger } = await makeSignedMandateBundle({
      maxTotal: '2.00',
      withChild: true,
      childMaxTotal: '1.00',
    });
    const receipt = makeReceipt({
      chain_hash: chainHash,
      root_jti: rootJti,
      path,
      ledger,
      currency: 'USD',
      total_spent: '1.50',
      spend_digests: ['e'.repeat(64)],
    });
    const res = await checkMandateBundleAgainstReceipt(json, receipt, 2000);
    expect(res.status).toBe('mismatch');
    if (res.status === 'mismatch') {
      expect(res.errors[0]).toContain('exceeds mandate max_total 1.00 USD');
    }
  });

  it('rejects a tampered JWS signature fail-closed', async () => {
    const { json, rootJti, path, chainHash, ledger } = await makeSignedMandateBundle();
    const parsed = JSON.parse(json) as { tokens: string[] };
    const tok = parsed.tokens[0]!;
    // Flip a character in the signature segment.
    const last = tok.slice(-1) === 'A' ? 'B' : 'A';
    parsed.tokens[0] = tok.slice(0, -1) + last;
    const receipt = makeReceipt({
      chain_hash: chainHash,
      root_jti: rootJti,
      path,
      ledger,
      currency: 'USD',
      total_spent: '0.25',
      spend_digests: [],
    });
    const res = await checkMandateBundleAgainstReceipt(JSON.stringify(parsed), receipt, 2000);
    expect(res.status).toBe('invalid');
    if (res.status === 'invalid') {
      expect(res.errors[0]).toContain('Ed25519 JWS signature verification failed');
    }
  });

  it('rejects an expired mandate chain and receipts missing mandate_provenance', async () => {
    const expired = await makeSignedMandateBundle({ exp: 1500 });
    const receipt = makeReceipt(null);
    const expRes = await checkMandateBundleAgainstReceipt(expired.json, receipt, 2000);
    expect(expRes.status).toBe('invalid');

    const fresh = await makeSignedMandateBundle({ exp: 5000 });
    const missingRes = await checkMandateBundleAgainstReceipt(fresh.json, receipt, 2000);
    expect(missingRes.status).toBe('mismatch');
    if (missingRes.status === 'mismatch') {
      expect(missingRes.errors[0]).toContain('missing mandate_provenance');
    }
  });
});
