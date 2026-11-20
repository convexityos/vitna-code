import type { Receipt } from './receipt';

export type RedactionLevel = 'none' | 'standard' | 'strict';

export interface RedactedReceiptEnvelope {
  redaction: 'standard' | 'strict';
  signature_stripped: true;
  note: string;
  receipt: Receipt;
}

function basenameOf(filePath: string): string {
  const normalized = filePath.replace(/\\/g, '/');
  const trimmed = normalized.replace(/\/+$/, '');
  const idx = trimmed.lastIndexOf('/');
  const base = idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
  return base || 'file';
}

/**
 * Formats a loaded receipt for local export at the requested redaction level,
 * matching `vitna receipt export` and `vitna-tui /export` field for field.
 *
 * Modifying any signed field invalidates the Ed25519 signature, so `standard`
 * and `strict` exports always strip `device_signature` and `device_public_key`
 * and wrap the payload in an explicit redaction envelope.
 */
export function formatReceiptExport(receipt: Receipt, rawText: string, level: RedactionLevel): string {
  if (level === 'none') {
    return rawText;
  }

  const cloned: Receipt = {
    ...receipt,
    workspace_fingerprint: '[REDACTED]',
    base_commit_sha: '[REDACTED]',
    changeset: {
      ...receipt.changeset,
      files_modified: receipt.changeset.files_modified.map((f, idx) => ({
        ...f,
        path: level === 'standard' ? `<redacted>/${basenameOf(f.path)}` : `redacted-file-${idx}`,
      })),
    },
    evidence_items:
      level === 'strict'
        ? receipt.evidence_items.map((ev) => ({
            ...ev,
            description: '[REDACTED]',
            artifact_digest: null,
          }))
        : receipt.evidence_items.map((ev) => ({ ...ev })),
    device_signature: '',
    device_public_key: null,
  };

  const envelope: RedactedReceiptEnvelope = {
    redaction: level,
    signature_stripped: true,
    note:
      level === 'standard'
        ? 'Redacted receipts cannot carry the original device signature because signed fields were modified.'
        : 'Strictly redacted receipts strip paths, evidence descriptions, artifact digests, and the original device signature.',
    receipt: cloned,
  };

  return JSON.stringify(envelope, null, 2);
}

/** Builds a safe download filename for an exported receipt. */
export function exportFilename(runId: string, level: RedactionLevel): string {
  const safeId = runId.replace(/[^A-Za-z0-9._-]/g, '_') || 'receipt';
  return level === 'none' ? `${safeId}.json` : `${safeId}.${level}.redacted.json`;
}
