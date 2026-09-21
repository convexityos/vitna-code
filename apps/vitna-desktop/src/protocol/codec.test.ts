import { describe, expect, it } from 'vitest';
import { decodeCommandPayload, decodeEnvelope, encodeCommand, encodeEvent } from './codec';
import { TYPE_URL_PREFIX, type ProtocolEnvelope } from './types';

const text = new TextEncoder();

function envelope(kind: string, payload: unknown, overrides: Partial<ProtocolEnvelope> = {}): ProtocolEnvelope {
  return {
    protocol_version_major: 1,
    protocol_version_minor: 0,
    schema_version: 1,
    type_url: TYPE_URL_PREFIX + kind,
    session_id: 's',
    run_id: 'r',
    sequence: 1,
    idempotency_key: '',
    payload: text.encode(typeof payload === 'string' ? payload : JSON.stringify(payload)),
    ...overrides,
  };
}

describe('decodeEnvelope', () => {
  it('reads the serde dialect: bytes as number arrays, integers as numbers', () => {
    const decoded = decodeEnvelope(
      envelope('ToolOutput', { tool_call_id: 'c1', stream: 'stdout', chunk: [104, 105] }),
    );
    expect(decoded.ok).toBe(true);
    if (decoded.ok && decoded.event.kind === 'ToolOutput') {
      expect(new TextDecoder().decode(decoded.event.body.chunk)).toBe('hi');
    }
  });

  it('reads the proto3 JSON dialect: bytes as base64, uint64 as a string', () => {
    const decoded = decodeEnvelope(
      envelope('ToolProposed', { tool_call_id: 'c1', raw_arguments_json: btoa('{"a":1}') }),
    );
    expect(decoded.ok && decoded.event.kind === 'ToolProposed').toBe(true);
    if (decoded.ok && decoded.event.kind === 'ToolProposed') {
      expect(new TextDecoder().decode(decoded.event.body.raw_arguments_json)).toBe('{"a":1}');
    }
    const usage = decodeEnvelope(envelope('UsageUpdated', { prompt_tokens: '9007199254740991' }));
    expect(usage.ok && usage.event.kind === 'UsageUpdated' && usage.event.body.prompt_tokens).toBe(
      Number.MAX_SAFE_INTEGER,
    );
  });

  it('gives a missing field its proto3 default', () => {
    const decoded = decodeEnvelope(envelope('ApprovalRequested', {}));
    expect(decoded).toEqual({
      ok: true,
      event: {
        kind: 'ApprovalRequested',
        body: {
          approval_id: '',
          tool_call_id: '',
          action_digest: '',
          description: '',
          executable_identity: '',
          canonical_cwd: '',
          environment_names: [],
          bound_mounts: [],
          timeout_ms: 0,
        },
      },
    });
    expect(decodeEnvelope(envelope('Diagnostic', '')).ok).toBe(true);
  });

  it('refuses a field of the wrong type instead of guessing', () => {
    const decoded = decodeEnvelope(envelope('ApprovalRequested', { action_digest: 42 }));
    expect(decoded).toEqual({
      ok: false,
      reason: 'malformed',
      detail: 'ApprovalRequested: action_digest: expected a string',
    });
    expect(decodeEnvelope(envelope('ToolFinished', { exit_code: 1.5 })).ok).toBe(false);
    expect(decodeEnvelope(envelope('ToolOutput', { chunk: [300] })).ok).toBe(false);
    expect(decodeEnvelope(envelope('UsageUpdated', { prompt_tokens: -1 })).ok).toBe(false);
    expect(decodeEnvelope(envelope('UsageUpdated', { prompt_tokens: 2 ** 60 })).ok).toBe(false);
    expect(decodeEnvelope(envelope('PlanChanged', [])).ok).toBe(false);
    expect(decodeEnvelope(envelope('PlanChanged', '{not json')).ok).toBe(false);
  });

  it('names what it cannot read rather than dropping it', () => {
    expect(decodeEnvelope(envelope('ReceiptGenerated', {}))).toEqual({
      ok: false,
      reason: 'unknown_type',
      detail: `${TYPE_URL_PREFIX}ReceiptGenerated`,
    });
    // What crates/orchestration actually emits today.
    expect(decodeEnvelope(envelope('x', {}, { type_url: 'vitna.v1.ApprovalRequested' })).ok).toBe(false);
    expect(decodeEnvelope(envelope('Diagnostic', {}, { protocol_version_major: 2 }))).toMatchObject({
      ok: false,
      reason: 'unsupported_version',
    });
  });

  it('reads an error code by name or by number', () => {
    const byName = decodeEnvelope(envelope('ErrorResponse', { code: 'ERROR_CODE_POLICY_DENIED' }));
    const byNumber = decodeEnvelope(envelope('ErrorResponse', { code: 5 }));
    expect(byName).toEqual(byNumber);
    expect(decodeEnvelope(envelope('ErrorResponse', { code: 99 })).ok).toBe(false);
  });
});

describe('commands', () => {
  it('round-trips through an envelope', () => {
    const env = encodeCommand(
      { kind: 'ApproveAction', body: { approval_id: 'a1', action_digest: 'd'.repeat(64), scope: 'once' } },
      { session_id: 's1', run_id: 'r1', sequence: 7, idempotency_key: 'k' },
    );
    expect(env.type_url).toBe(`${TYPE_URL_PREFIX}ApproveAction`);
    expect(env.sequence).toBe(7);
    expect(decodeCommandPayload(env)).toEqual({
      kind: 'ApproveAction',
      body: { approval_id: 'a1', action_digest: 'd'.repeat(64), scope: 'once' },
    });
  });

  it('writes event bytes the serde way so the decoder reads them back', () => {
    const env = encodeEvent(
      { kind: 'ToolOutput', body: { tool_call_id: 'c', stream: 'stderr', chunk: new Uint8Array([0, 255]) } },
      { session_id: 's', run_id: 'r', sequence: 1, idempotency_key: '' },
    );
    expect(new TextDecoder().decode(env.payload)).toBe('{"tool_call_id":"c","stream":"stderr","chunk":[0,255]}');
    const back = decodeEnvelope(env);
    expect(back.ok && back.event.kind === 'ToolOutput' && Array.from(back.event.body.chunk)).toEqual([0, 255]);
  });
});
