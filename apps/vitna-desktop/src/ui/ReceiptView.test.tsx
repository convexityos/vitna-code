// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  encodeBase64Url,
  MANDATE_CONFIG_SCHEMA_V1,
  MANDATE_JWS_TYP,
  MANDATE_PROTOCOL_VERSION,
  publicKeyBytesToDidKey,
} from '../receipt/mandate';
import { bytesToHex } from '../receipt/verify';
import { ReceiptView } from './ReceiptView';

afterEach(cleanup);

async function makeSignedBundleAndReceipt() {
  const rootPair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  const rootPub = new Uint8Array(await crypto.subtle.exportKey('raw', rootPair.publicKey));
  const rootDid = publicKeyBytesToDidKey(rootPub);

  const agentPair = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  const agentPub = new Uint8Array(await crypto.subtle.exportKey('raw', agentPair.publicKey));
  const agentDid = publicKeyBytesToDidKey(agentPub);

  const now = Math.floor(Date.now() / 1000);
  const rootJti = 'mnd-ui-root-1';
  const ledger = 'https://ledger.vitna.local';
  const header = { alg: 'EdDSA', typ: MANDATE_JWS_TYP, kid: `${rootDid}#${rootDid.slice('did:key:'.length)}` };
  const claims = {
    v: MANDATE_PROTOCOL_VERSION,
    typ: 'mandate',
    jti: rootJti,
    iss: rootDid,
    sub: agentDid,
    par: null,
    iat: now - 60,
    nbf: now - 60,
    exp: now + 3600,
    ledger,
    cav: { max_total: { cur: 'USD', amt: '2.00' }, depth: 1, delegable: true },
  };
  const hSeg = encodeBase64Url(new TextEncoder().encode(JSON.stringify(header)));
  const pSeg = encodeBase64Url(new TextEncoder().encode(JSON.stringify(claims)));
  const signingInput = `${hSeg}.${pSeg}`;
  const sig = await crypto.subtle.sign({ name: 'Ed25519' }, rootPair.privateKey, new TextEncoder().encode(signingInput));
  const token = `${signingInput}.${encodeBase64Url(new Uint8Array(sig))}`;
  const hashBuf = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
  const chainHash = `sha256:${bytesToHex(new Uint8Array(hashBuf))}`;

  const bundleJson = JSON.stringify({
    schema_version: MANDATE_CONFIG_SCHEMA_V1,
    tokens: [token],
    trusted_root_issuers: [rootDid],
    ledger,
    currency: 'USD',
  });

  const receiptA = {
    schema_version: 'vitna-run-receipt-v1',
    run_id: 'run-ui-a',
    session_id: 'sess-ui',
    workspace_fingerprint: 'a'.repeat(64),
    base_commit_sha: 'b'.repeat(40),
    model_selection: {
      provider: 'vitna-runtime',
      model_sku: 'vitna/code',
      routing_reason: 'runtime_lane:vitna/code',
    },
    event_hash_chain_root: 'c'.repeat(64),
    isolation_label: 'guarded',
    completion_state: 'completed_with_evidence',
    evidence_items: [],
    changeset: {
      files_modified: [{ path: 'src/app.ts', preimage_hash: '0'.repeat(64), postimage_hash: '1'.repeat(64) }],
      diff_digest: 'd'.repeat(64),
    },
    runner_execution_statements: [],
    egress_calls: [],
    mandate_provenance: {
      chain_hash: chainHash,
      root_jti: rootJti,
      path: [rootJti],
      ledger,
      currency: 'USD',
      total_spent: '0.50',
      spend_digests: [],
    },
    device_signature: '',
  };

  const receiptB = {
    ...receiptA,
    run_id: 'run-ui-b',
    isolation_label: 'full_access',
    model_selection: {
      provider: 'openai',
      model_sku: 'gpt-5',
      routing_reason: 'direct',
    },
    egress_calls: [
      {
        target_domain: 'api.vendor.example',
        method: 'GET',
        redacted_url: 'https://api.vendor.example/ping',
        request_sha256: '1'.repeat(64),
        status_code: 200,
        response_sha256: '2'.repeat(64),
        bytes_sent: 10,
        bytes_received: 20,
        latency_ms: 12,
      },
    ],
  };

  return {
    bundleJson,
    receiptAJson: JSON.stringify(receiptA),
    receiptBJson: JSON.stringify(receiptB),
  };
}

describe('ReceiptView mandate verification and diff UI', () => {
  it('verifies an am/1 mandate bundle and renders side-by-side receipt comparison with regression alerts', async () => {
    const { bundleJson, receiptAJson, receiptBJson } = await makeSignedBundleAndReceipt();
    render(<ReceiptView isSample={false} />);

    const receiptInput = screen.getByPlaceholderText('Or paste the JSON here.');
    fireEvent.change(receiptInput, { target: { value: receiptAJson } });

    const mandateInput = screen.getByLabelText(/Agent Mandate bundle/i);
    fireEvent.change(mandateInput, { target: { value: bundleJson } });

    await waitFor(() => {
      expect(screen.getByText(/Mandate \(am\/1\) verified \(mnd-ui-root-1\)/)).toBeTruthy();
    });

    const compareInput = screen.getByLabelText(/Compare against second receipt/i);
    fireEvent.change(compareInput, { target: { value: receiptBJson } });

    const comparison = screen.getByRole('region', { name: 'Receipt comparison' });
    expect(comparison.textContent).toContain('[REGRESSION]');
    expect(comparison.textContent).toContain('[CHANGED]');
    expect(comparison.textContent).toContain('api.vendor.example');
  });
});
