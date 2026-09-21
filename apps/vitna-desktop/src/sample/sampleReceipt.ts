/**
 * A receipt for the sample run, signed for real with a key made in this
 * window a moment earlier. DEV only, like the rest of src/sample.
 *
 * It exists so the receipt view can be exercised end to end: the signature
 * checks against the key offered beside it, and editing any signed field in
 * the text makes the same check fail. It proves this window's
 * canonicalization agrees with itself; agreement with the Rust signer is what
 * src/receipt/receipt.test.ts pins, byte for byte.
 */

import { canonicalBytes, signedValue, type Receipt } from '../receipt/receipt';
import { bytesToHex } from '../receipt/verify';
import { PYTEST_OUTPUT, SAMPLE_WORKSPACE, SERVICE_AFTER, SERVICE_BEFORE, SERVICE_DIFF, SERVICE_PATH } from './content';
import { sha256Hex } from './sampleTransport';

/** crates/receipts compute_event_merkle_root: pairwise SHA-256, an odd node paired with itself. */
export async function merkleRoot(leavesHex: string[]): Promise<string> {
  if (leavesHex.length === 0) return '0'.repeat(64);
  let level = leavesHex;
  while (level.length > 1) {
    const next: string[] = [];
    for (let i = 0; i < level.length; i += 2) {
      const left = level[i] as string;
      const right = level[i + 1] ?? left;
      next.push(await sha256Hex(hexBytes(left + right)));
    }
    level = next;
  }
  return level[0] as string;
}

function hexBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export type SampleReceipt = { ok: true; json: string; publicKeyHex: string } | { ok: false; reason: string };

export async function buildSampleReceipt(): Promise<SampleReceipt> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return { ok: false, reason: 'This window has no WebCrypto.' };

  let keys: CryptoKeyPair;
  try {
    keys = (await subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  } catch {
    return { ok: false, reason: 'This browser cannot make Ed25519 keys, so it cannot sign a sample receipt.' };
  }

  const events = [
    'RunStateChanged:queued',
    'ToolFinished:read_file',
    'ToolFinished:search_code',
    'ApprovalRequested:apply_patch',
    'ToolFinished:apply_patch',
    'ApprovalRequested:run_command',
    'ToolFinished:run_command',
    'RunStateChanged:completed',
  ];
  const leaves = await Promise.all(events.map((event) => sha256Hex(event)));
  const commit = new Uint8Array(await subtle.digest('SHA-1', new TextEncoder().encode(SERVICE_BEFORE)));

  const receipt: Receipt = {
    schema_version: 'vitna-run-receipt-v1',
    run_id: 'run-sample-1',
    session_id: 'sess-sample',
    workspace_fingerprint: await sha256Hex(SAMPLE_WORKSPACE),
    base_commit_sha: bytesToHex(commit),
    model_selection: {
      provider: 'anthropic',
      model_sku: 'claude-sonnet-5',
      routing_reason: 'session policy: default coding model',
      policy_digest: null,
    },
    event_hash_chain_root: await merkleRoot(leaves),
    isolation_label: 'guarded',
    completion_state: 'completed_with_unknowns',
    evidence_items: [
      {
        evidence_id: 'ev-tests',
        grade: 'broker_observed',
        description: 'python -m pytest tests/test_rate_limit.py -q exited 0: 3 passed',
        artifact_digest: await sha256Hex(PYTEST_OUTPUT.join('')),
      },
      {
        evidence_id: 'ev-latency',
        grade: 'model_reported',
        description: 'The 50 ms Redis bound was not load-tested against real Redis latency.',
        artifact_digest: null,
      },
    ],
    changeset: {
      files_modified: [
        {
          path: SERVICE_PATH,
          preimage_hash: await sha256Hex(SERVICE_BEFORE),
          postimage_hash: await sha256Hex(SERVICE_AFTER),
        },
      ],
      diff_digest: await sha256Hex(SERVICE_DIFF),
    },
    // The protocol does not say what a runner statement signs, so the sample
    // leaves them out rather than inventing one.
    runner_execution_statements: [],
    child_receipt_roots: [],
    device_signature: '',
  };

  const signature = new Uint8Array(await subtle.sign({ name: 'Ed25519' }, keys.privateKey, canonicalBytes(receipt)));
  const publicKey = new Uint8Array(await subtle.exportKey('raw', keys.publicKey));

  const value = signedValue(receipt) as Record<string, unknown>;
  value.device_signature = bytesToHex(signature);
  return { ok: true, json: JSON.stringify(value, null, 2), publicKeyHex: bytesToHex(publicKey) };
}
