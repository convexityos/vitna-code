import { beforeAll, describe, expect, it } from 'vitest';
import { canonicalBytes, readReceipt, type Receipt } from './receipt';
import { bytesToHex, checkDeviceSignature, hexToBytes, signatureShape } from './verify';

const base = {
  schema_version: 'vitna-run-receipt-v1',
  run_id: 'run-7',
  session_id: 'sess-7',
  workspace_fingerprint: 'fp',
  base_commit_sha: 'c'.repeat(40),
  model_selection: { provider: 'anthropic', model_sku: 'claude-sonnet-5', routing_reason: 'policy' },
  event_hash_chain_root: 'e'.repeat(64),
  isolation_label: 'guarded',
  completion_state: 'completed_with_unknowns',
  evidence_items: [],
  changeset: { files_modified: [{ path: 'a.py', preimage_hash: '1', postimage_hash: '2' }], diff_digest: 'd' },
  runner_execution_statements: [],
  device_signature: '',
};

let keys: CryptoKeyPair;
let publicKeyHex: string;
let signedText: string;

beforeAll(async () => {
  keys = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
  publicKeyHex = bytesToHex(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey)));
  const receipt = readReceipt(JSON.stringify(base)).receipt as Receipt;
  const signature = new Uint8Array(await crypto.subtle.sign({ name: 'Ed25519' }, keys.privateKey, canonicalBytes(receipt)));
  signedText = JSON.stringify({ ...base, device_signature: bytesToHex(signature) });
});

const check = (text: string, key: string) => checkDeviceSignature(readReceipt(text).receipt as Receipt, key);

describe('checkDeviceSignature', () => {
  it('accepts the key that signed it', async () => {
    expect(await check(signedText, publicKeyHex)).toEqual({ status: 'valid' });
    expect(await check(signedText, publicKeyHex.toUpperCase())).toEqual({ status: 'valid' });
  });

  it('fails when any signed field changes', async () => {
    const tampered = signedText.replace('"diff_digest":"d"', '"diff_digest":"x"');
    expect(await check(tampered, publicKeyHex)).toEqual({ status: 'invalid' });
    const reordered = JSON.stringify({ ...JSON.parse(signedText), completion_state: 'completed_with_evidence' });
    expect(await check(reordered, publicKeyHex)).toEqual({ status: 'invalid' });
  });

  it('still passes when only unsigned keys change, which is why they are listed', async () => {
    const decorated = JSON.stringify({ ...JSON.parse(signedText), verified_by: 'anyone' });
    expect(await check(decorated, publicKeyHex)).toEqual({ status: 'valid' });
    expect(readReceipt(decorated).uncovered).toEqual(['verified_by']);
  });

  it('fails against a different key', async () => {
    const other = (await crypto.subtle.generateKey({ name: 'Ed25519' }, true, ['sign', 'verify'])) as CryptoKeyPair;
    const otherHex = bytesToHex(new Uint8Array(await crypto.subtle.exportKey('raw', other.publicKey)));
    expect(await check(signedText, otherHex)).toEqual({ status: 'invalid' });
  });

  it('says "not checked" without a key, never "valid"', async () => {
    expect(await check(signedText, '')).toEqual({ status: 'not_checked' });
    expect(await check(signedText, '   ')).toEqual({ status: 'not_checked' });
  });

  it('names placeholder and malformed signatures before any key is needed', async () => {
    const zeros = JSON.stringify({ ...base, device_signature: '0'.repeat(128) });
    expect(await check(zeros, '')).toEqual({ status: 'placeholder' });
    expect(await check(JSON.stringify(base), publicKeyHex)).toEqual({ status: 'no_signature' });
    const short = JSON.stringify({ ...base, device_signature: 'ab' });
    expect((await check(short, publicKeyHex)).status).toBe('bad_signature');
  });

  it('refuses a key of the wrong size', async () => {
    expect((await check(signedText, 'abcd')).status).toBe('bad_key');
    expect((await check(signedText, 'zz'.repeat(32))).status).toBe('bad_key');
  });
});

describe('hex helpers', () => {
  it('round-trip and reject odd or non-hex input', () => {
    expect(bytesToHex(hexToBytes('00ff10') as Uint8Array)).toBe('00ff10');
    expect(hexToBytes('0x0a')).toEqual(new Uint8Array([10]));
    expect(hexToBytes('abc')).toBeNull();
    expect(hexToBytes('gg')).toBeNull();
    expect(signatureShape('')).toBe('empty');
    expect(signatureShape('00'.repeat(64))).toBe('placeholder');
    expect(signatureShape('01'.repeat(64))).toBe('ed25519');
    expect(signatureShape('01'.repeat(63))).toBe('malformed');
  });
});
