import { describe, expect, it } from 'vitest';
import { diffReceipts } from './diff';
import type { Receipt } from './receipt';

function baseReceipt(overrides: Partial<Receipt> = {}): Receipt {
  return {
    schema_version: 'vitna-run-receipt-v1',
    run_id: 'run-a',
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
    evidence_items: [
      {
        evidence_id: 'ev-1',
        grade: 'sandbox_captured',
        description: 'cargo test',
        artifact_digest: null,
      },
    ],
    changeset: {
      files_modified: [
        { path: 'src/a.rs', preimage_hash: '0'.repeat(64), postimage_hash: '1'.repeat(64) },
        { path: 'src/shared.rs', preimage_hash: '0'.repeat(64), postimage_hash: '2'.repeat(64) },
        { path: 'src/same.rs', preimage_hash: '0'.repeat(64), postimage_hash: '3'.repeat(64) },
      ],
      diff_digest: 'd'.repeat(64),
    },
    runner_execution_statements: [],
    egress_calls: [],
    child_receipt_roots: [],
    device_public_key: null,
    mandate_provenance: {
      chain_hash: 'sha256:' + 'f'.repeat(64),
      root_jti: 'mnd-1',
      path: ['mnd-1'],
      ledger: 'https://ledger.vitna.local',
      currency: 'USD',
      total_spent: '0.25',
      spend_digests: [],
    },
    device_signature: '',
    ...overrides,
  };
}

describe('diffReceipts', () => {
  it('flags isolation regression, model change, file deltas, new egress domains, and mandate spend', () => {
    const a = baseReceipt();
    const b = baseReceipt({
      run_id: 'run-b',
      isolation_label: 'full_access',
      completion_state: 'completed_with_unknowns',
      model_selection: {
        provider: 'openai',
        model_sku: 'gpt-5',
        routing_reason: 'direct',
        policy_digest: null,
      },
      evidence_items: [],
      changeset: {
        files_modified: [
          { path: 'src/shared.rs', preimage_hash: '0'.repeat(64), postimage_hash: '9'.repeat(64) },
          { path: 'src/same.rs', preimage_hash: '0'.repeat(64), postimage_hash: '3'.repeat(64) },
          { path: 'src/new.rs', preimage_hash: '0'.repeat(64), postimage_hash: '4'.repeat(64) },
        ],
        diff_digest: 'e'.repeat(64),
      },
      egress_calls: [
        {
          target_domain: 'api.example.com',
          method: 'GET',
          redacted_url: 'https://api.example.com/v1',
          request_sha256: '1'.repeat(64),
          status_code: 200,
          response_sha256: '2'.repeat(64),
          bytes_sent: 10,
          bytes_received: 50,
          latency_ms: 25,
        },
      ],
      mandate_provenance: {
        chain_hash: 'sha256:' + 'f'.repeat(64),
        root_jti: 'mnd-1',
        path: ['mnd-1'],
        ledger: 'https://ledger.vitna.local',
        currency: 'USD',
        total_spent: '1.75',
        spend_digests: [],
      },
    });

    const diff = diffReceipts(a, b);
    expect(diff.model.changed).toBe(true);
    expect(diff.isolation).toEqual({
      changed: true,
      regression: true,
      a: 'guarded',
      b: 'full_access',
    });
    expect(diff.files).toEqual({
      onlyInA: ['src/a.rs'],
      onlyInB: ['src/new.rs'],
      postimageChanged: ['src/shared.rs'],
      unchanged: ['src/same.rs'],
    });
    expect(diff.evidence).toEqual({
      countA: 1,
      countB: 0,
      highestGradeA: 'sandbox_captured',
      highestGradeB: 'none',
    });
    expect(diff.egress.newDomains).toEqual(['api.example.com']);
    expect(diff.mandate.spentA).toBe('0.25 USD');
    expect(diff.mandate.spentB).toBe('1.75 USD');
  });

  it('does not flag regression when isolation strengthens (guarded -> strong)', () => {
    const a = baseReceipt({ isolation_label: 'guarded' });
    const b = baseReceipt({ run_id: 'run-b', isolation_label: 'strong' });
    const diff = diffReceipts(a, b);
    expect(diff.isolation.changed).toBe(true);
    expect(diff.isolation.regression).toBe(false);
  });
});
