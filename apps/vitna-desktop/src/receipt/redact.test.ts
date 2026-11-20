import { describe, expect, it } from 'vitest';
import type { Receipt } from './receipt';
import { exportFilename, formatReceiptExport } from './redact';

const SAMPLE: Receipt = {
  schema_version: 'vitna-run-receipt-v1',
  run_id: 'run-42',
  session_id: 'sess-7',
  workspace_fingerprint: 'sha256:workspace-fp-secret',
  base_commit_sha: 'abcdef0123456789abcdef0123456789abcdef01',
  model_selection: {
    provider: 'anthropic',
    model_sku: 'claude-sonnet-4-5',
    routing_reason: 'direct_byok:anthropic',
    policy_digest: null,
  },
  event_hash_chain_root: 'root-hash-1',
  isolation_label: 'guarded',
  completion_state: 'completed_with_evidence',
  evidence_items: [
    {
      evidence_id: 'ev-1',
      grade: 'sandbox_captured',
      description: 'cargo test passed on /home/alice/secret-repo',
      artifact_digest: 'sha256:artifact-digest',
    },
  ],
  changeset: {
    files_modified: [
      {
        path: 'crates/secret/src/lib.rs',
        preimage_hash: 'pre-1',
        postimage_hash: 'post-1',
      },
    ],
    diff_digest: 'diff-1',
  },
  runner_execution_statements: [],
  egress_calls: [],
  child_receipt_roots: [],
  device_public_key: '11'.repeat(32),
  mandate_provenance: null,
  device_signature: 'aa'.repeat(64),
};

describe('formatReceiptExport', () => {
  it('returns verbatim raw text for none redaction', () => {
    const raw = '{"verbatim":true}';
    expect(formatReceiptExport(SAMPLE, raw, 'none')).toBe(raw);
  });

  it('redacts workspace fingerprint, commit, and parent dirs and strips signature in standard mode', () => {
    const out = JSON.parse(formatReceiptExport(SAMPLE, '{}', 'standard')) as {
      redaction: string;
      signature_stripped: boolean;
      receipt: Receipt;
    };
    expect(out.redaction).toBe('standard');
    expect(out.signature_stripped).toBe(true);
    expect(out.receipt.workspace_fingerprint).toBe('[REDACTED]');
    expect(out.receipt.base_commit_sha).toBe('[REDACTED]');
    expect(out.receipt.changeset.files_modified[0]?.path).toBe('<redacted>/lib.rs');
    expect(out.receipt.evidence_items[0]?.description).toContain('cargo test');
    expect(out.receipt.device_signature).toBe('');
    expect(out.receipt.device_public_key).toBeNull();
  });

  it('strips paths, evidence descriptions, artifact digests, and signature in strict mode', () => {
    const out = JSON.parse(formatReceiptExport(SAMPLE, '{}', 'strict')) as {
      redaction: string;
      signature_stripped: boolean;
      receipt: Receipt;
    };
    expect(out.redaction).toBe('strict');
    expect(out.signature_stripped).toBe(true);
    expect(out.receipt.changeset.files_modified[0]?.path).toBe('redacted-file-0');
    expect(out.receipt.evidence_items[0]?.description).toBe('[REDACTED]');
    expect(out.receipt.evidence_items[0]?.artifact_digest).toBeNull();
    expect(out.receipt.device_signature).toBe('');
    expect(out.receipt.device_public_key).toBeNull();
  });

  it('sanitizes export filenames', () => {
    expect(exportFilename('run-42', 'none')).toBe('run-42.json');
    expect(exportFilename('../evil/run', 'standard')).toBe('.._evil_run.standard.redacted.json');
    expect(exportFilename('run-42', 'strict')).toBe('run-42.strict.redacted.json');
  });
});
