import { describe, expect, it } from 'vitest';
import { encodeFrame, envelopeToJson, FrameDecoder } from './frame';
import { MAX_FRAME_SIZE_BYTES, type ProtocolEnvelope } from './types';

const sample: ProtocolEnvelope = {
  protocol_version_major: 1,
  protocol_version_minor: 0,
  schema_version: 1,
  type_url: 'type.vitna.ai/vitna.protocol.v1.SubmitTurn',
  session_id: 'session-001',
  run_id: 'run-101',
  sequence: 1,
  idempotency_key: 'idem-key-99',
  payload: new TextEncoder().encode('{"p":1}'),
};

describe('frames', () => {
  it('writes the layout crates/protocol writes: 4-byte big-endian length, then serde JSON in struct order', () => {
    const json =
      '{"protocol_version_major":1,"protocol_version_minor":0,"schema_version":1,' +
      '"type_url":"type.vitna.ai/vitna.protocol.v1.SubmitTurn","session_id":"session-001",' +
      '"run_id":"run-101","sequence":1,"idempotency_key":"idem-key-99","payload":[123,34,112,34,58,49,125]}';
    expect(envelopeToJson(sample)).toBe(json);
    const frame = encodeFrame(sample);
    const length = json.length;
    expect(Array.from(frame.subarray(0, 4))).toEqual([
      (length >>> 24) & 0xff,
      (length >>> 16) & 0xff,
      (length >>> 8) & 0xff,
      length & 0xff,
    ]);
    expect(new TextDecoder().decode(frame.subarray(4))).toBe(json);
  });

  it('reassembles a frame delivered one byte at a time', () => {
    const decoder = new FrameDecoder();
    const frame = encodeFrame(sample);
    const out: ProtocolEnvelope[] = [];
    for (const byte of frame) {
      const batch = decoder.push(Uint8Array.of(byte));
      expect(batch.error).toBeNull();
      out.push(...batch.envelopes);
    }
    expect(out).toEqual([sample]);
  });

  it('splits several frames that arrive together', () => {
    const a = encodeFrame(sample);
    const b = encodeFrame({ ...sample, sequence: 2 });
    const joined = new Uint8Array(a.length + b.length);
    joined.set(a);
    joined.set(b, a.length);
    const batch = new FrameDecoder().push(joined);
    expect(batch.envelopes.map((e) => e.sequence)).toEqual([1, 2]);
  });

  it('drops the connection on an oversized frame, keeping the good frames ahead of it', () => {
    const decoder = new FrameDecoder();
    const good = encodeFrame(sample);
    const bad = new Uint8Array(4);
    new DataView(bad.buffer).setUint32(0, MAX_FRAME_SIZE_BYTES + 1, false);
    const joined = new Uint8Array(good.length + 4);
    joined.set(good);
    joined.set(bad, good.length);

    const batch = decoder.push(joined);
    expect(batch.envelopes).toHaveLength(1);
    expect(batch.error?.message).toMatch(/exceeds/);
    expect(decoder.poisoned).toBe(true);
    // Nothing is read after a bad length, not even a well-formed frame.
    expect(decoder.push(good)).toEqual({ envelopes: [], error: batch.error });
  });

  it('refuses a frame whose body is not an envelope', () => {
    const body = new TextEncoder().encode('{"type_url":5}');
    const frame = new Uint8Array(4 + body.length);
    new DataView(frame.buffer).setUint32(0, body.length, false);
    frame.set(body, 4);
    expect(new FrameDecoder().push(frame).error).not.toBeNull();
  });
});
