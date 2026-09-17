import { describe, expect, it } from 'vitest';
import { canonicalBytes, canonicalJson, readReceipt, signedValue, type Receipt } from './receipt';

/**
 * The receipt from crates/receipts' own unit test
 * (test_receipt_canonicalization_and_signing), written as a file would carry it.
 */
const RUST_TEST_RECEIPT = {
  schema_version: 'vitna-run-receipt-v1',
  run_id: 'run-001',
  session_id: 'sess-001',
  workspace_fingerprint: 'ws-fp-123',
  base_commit_sha: 'abcd1234abcd1234abcd1234abcd1234abcd1234',
  model_selection: {
    provider: 'anthropic',
    model_sku: 'claude-3-7-sonnet',
    routing_reason: 'pinned_profile',
    policy_digest: 'policy-hash-456',
  },
  event_hash_chain_root: 'merkle-root-789',
  isolation_label: 'guarded',
  completion_state: 'completed_with_evidence',
  evidence_items: [
    {
      evidence_id: 'ev-1',
      grade: 'sandbox_captured',
      description: 'Targeted pytest passed cleanly',
      artifact_digest: 'art-hash-111',
    },
  ],
  changeset: {
    files_modified: [{ path: 'src/main.rs', preimage_hash: 'pre-1', postimage_hash: 'post-1' }],
    diff_digest: 'diff-hash-222',
  },
  runner_execution_statements: [{ action_id: 'act-1', statement_digest: 'stmt-digest-1', signature: 'sig-1' }],
  device_signature: 'aa'.repeat(64),
};

/**
 * What VitnaRunReceiptV1::to_canonical_bytes produces for that receipt,
 * derived by hand from canonicalize_value: keys sorted byte-wise at every
 * level, no whitespace, device_signature emptied, and the empty
 * child_receipt_roots left out by its skip_serializing_if. No Rust toolchain
 * ran to produce it (the workspace does not compile as committed); this pins
 * the reading of that function, so a change on either side is a visible diff.
 */
const RUST_TEST_CANONICAL =
  '{"base_commit_sha":"abcd1234abcd1234abcd1234abcd1234abcd1234",' +
  '"changeset":{"diff_digest":"diff-hash-222","files_modified":[{"path":"src/main.rs","postimage_hash":"post-1","preimage_hash":"pre-1"}]},' +
  '"completion_state":"completed_with_evidence",' +
  '"device_signature":"",' +
  '"event_hash_chain_root":"merkle-root-789",' +
  '"evidence_items":[{"artifact_digest":"art-hash-111","description":"Targeted pytest passed cleanly","evidence_id":"ev-1","grade":"sandbox_captured"}],' +
  '"isolation_label":"guarded",' +
  '"model_selection":{"model_sku":"claude-3-7-sonnet","policy_digest":"policy-hash-456","provider":"anthropic","routing_reason":"pinned_profile"},' +
  '"run_id":"run-001",' +
  '"runner_execution_statements":[{"action_id":"act-1","signature":"sig-1","statement_digest":"stmt-digest-1"}],' +
  '"schema_version":"vitna-run-receipt-v1",' +
  '"session_id":"sess-001",' +
  '"workspace_fingerprint":"ws-fp-123"}';

const read = (value: unknown) => readReceipt(JSON.stringify(value));

describe('canonical bytes', () => {
  it("match the Rust signer's canonical form for the Rust test receipt", () => {
    const reading = read(RUST_TEST_RECEIPT);
    expect(reading.schemaErrors).toEqual([]);
    expect(reading.rustMissing).toEqual([]);
    expect(new TextDecoder().decode(canonicalBytes(reading.receipt as Receipt))).toBe(RUST_TEST_CANONICAL);
  });

  it('leave out what serde leaves out, and keep an empty Some', () => {
    const noDigest = read({
      ...RUST_TEST_RECEIPT,
      model_selection: { ...RUST_TEST_RECEIPT.model_selection, policy_digest: null },
      child_receipt_roots: ['root-a'],
    }).receipt as Receipt;
    const text = canonicalJson(signedValue(noDigest));
    expect(text).not.toContain('policy_digest');
    expect(text).toContain('"child_receipt_roots":["root-a"]');

    const emptyDigest = read({
      ...RUST_TEST_RECEIPT,
      model_selection: { ...RUST_TEST_RECEIPT.model_selection, policy_digest: '' },
    }).receipt as Receipt;
    expect(canonicalJson(signedValue(emptyDigest))).toContain('"policy_digest":""');
  });

  it('escape strings the way serde_json does', () => {
    expect(canonicalJson('quote " slash \\ newline \n tab \t unit  del ')).toBe(
      '"quote \\" slash \\\\ newline \\n tab \\t unit \\u001f del "',
    );
    // serde_json does not escape non-ASCII or the forward slash.
    expect(canonicalJson('café / \u{1f600}')).toBe('"café / \u{1f600}"');
  });

  it('drop keys the struct does not declare, and say so', () => {
    const reading = read({
      ...RUST_TEST_RECEIPT,
      note: 'unsigned text',
      model_selection: { ...RUST_TEST_RECEIPT.model_selection, tier: 'pro' },
      evidence_items: [{ ...RUST_TEST_RECEIPT.evidence_items[0], verified: true }],
    });
    expect(reading.uncovered).toEqual(['note', 'model_selection.tier', 'evidence_items[0].verified']);
    expect(new TextDecoder().decode(canonicalBytes(reading.receipt as Receipt))).toBe(RUST_TEST_CANONICAL);
  });
});

describe('reading a receipt', () => {
  it('reports schema breaks by path', () => {
    const reading = read({
      ...RUST_TEST_RECEIPT,
      schema_version: 'v2',
      isolation_label: 'sandboxed',
      run_id: 7,
      evidence_items: [{ evidence_id: 'e', grade: 'vibes', description: 'd' }],
    });
    expect(reading.schemaErrors).toEqual([
      'schema_version must be "vitna-run-receipt-v1"',
      'isolation_label "sandboxed" is not one of read_only, guarded, strong, full_access',
      'evidence_items[0].grade "vibes" is not a known grade',
      'run_id must be a string',
    ]);
  });

  it('separates what the schema allows from what the Rust verifier can load', () => {
    const { evidence_items: _e, changeset: _c, runner_execution_statements: _r, ...minimal } = RUST_TEST_RECEIPT;
    const reading = read(minimal);
    expect(reading.schemaErrors).toEqual([]);
    expect(reading.rustMissing).toEqual(['evidence_items', 'changeset', 'runner_execution_statements']);
  });

  it('requires the signature field by schema and not by struct', () => {
    const { device_signature: _d, ...unsigned } = RUST_TEST_RECEIPT;
    const reading = read(unsigned);
    expect(reading.schemaErrors).toEqual(['device_signature is required']);
    expect(reading.rustMissing).toEqual([]);
  });

  it('refuses text that is not a receipt at all', () => {
    expect(readReceipt('nope').receipt).toBeNull();
    expect(readReceipt('[]').schemaErrors).toEqual(['a receipt is a JSON object']);
  });

  it('flags a string serde_json would refuse to read', () => {
    const reading = readReceipt(JSON.stringify(RUST_TEST_RECEIPT).replace('run-001', 'run-\\ud800'));
    expect(reading.schemaErrors).toEqual(['run_id is not valid Unicode']);
  });
});
