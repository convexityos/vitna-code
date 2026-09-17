/**
 * Wire framing, as crates/protocol/src/lib.rs implements it today: a 4-byte
 * big-endian length, then the envelope serialized with serde_json, whose
 * `payload: Vec<u8>` is a JSON array of numbers.
 *
 * ADR-0005 decides a serialized Protobuf envelope instead. The two disagree,
 * no crate compiles the .proto files, and this file follows the code because
 * the code is what a bridge would actually be handed. When the daemon settles
 * the question, this is the one file to change.
 */

import { bytesFromJson, DecodeError } from './codec';
import { MAX_FRAME_SIZE_BYTES, type ProtocolEnvelope } from './types';

export class FrameError extends Error {
  override readonly name = 'FrameError';
}

const encoder = new TextEncoder();

export function envelopeToJson(envelope: ProtocolEnvelope): string {
  // Field order follows the Rust struct, so a byte-for-byte comparison with
  // serde_json output holds.
  return JSON.stringify({
    protocol_version_major: envelope.protocol_version_major,
    protocol_version_minor: envelope.protocol_version_minor,
    schema_version: envelope.schema_version,
    type_url: envelope.type_url,
    session_id: envelope.session_id,
    run_id: envelope.run_id,
    sequence: envelope.sequence,
    idempotency_key: envelope.idempotency_key,
    payload: Array.from(envelope.payload),
  });
}

function field<T>(o: Record<string, unknown>, key: string, check: (v: unknown) => v is T): T {
  const v = o[key];
  if (!check(v)) throw new FrameError(`envelope.${key} is missing or has the wrong type`);
  return v;
}

const isUint = (v: unknown): v is number =>
  typeof v === 'number' && Number.isInteger(v) && v >= 0 && v <= Number.MAX_SAFE_INTEGER;
const isString = (v: unknown): v is string => typeof v === 'string';

export function envelopeFromJson(text: string): ProtocolEnvelope {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new FrameError('frame body is not JSON');
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new FrameError('frame body is not an envelope object');
  }
  const o = parsed as Record<string, unknown>;
  let payload: Uint8Array;
  try {
    payload = bytesFromJson(o.payload, 'payload');
  } catch (error) {
    throw new FrameError(error instanceof DecodeError ? error.message : 'payload is unreadable');
  }
  return {
    protocol_version_major: field(o, 'protocol_version_major', isUint),
    protocol_version_minor: field(o, 'protocol_version_minor', isUint),
    schema_version: field(o, 'schema_version', isUint),
    type_url: field(o, 'type_url', isString),
    session_id: field(o, 'session_id', isString),
    run_id: field(o, 'run_id', isString),
    sequence: field(o, 'sequence', isUint),
    idempotency_key: field(o, 'idempotency_key', isString),
    payload,
  };
}

export function encodeFrame(envelope: ProtocolEnvelope): Uint8Array {
  const body = encoder.encode(envelopeToJson(envelope));
  if (body.length > MAX_FRAME_SIZE_BYTES) {
    throw new FrameError(`frame of ${body.length} bytes exceeds the ${MAX_FRAME_SIZE_BYTES} byte limit`);
  }
  const frame = new Uint8Array(4 + body.length);
  new DataView(frame.buffer).setUint32(0, body.length, false);
  frame.set(body, 4);
  return frame;
}

/**
 * Splits an arbitrary byte stream into envelopes. A bridge may hand over
 * bytes in any chunking, so a frame can arrive split across calls or several
 * can arrive in one.
 *
 * An oversized or unreadable frame poisons the decoder: ADR-0005 admission
 * step 1 drops the connection rather than trying to resynchronize, because
 * after a bad length there is no trustworthy boundary left to find. The good
 * frames that arrived ahead of the bad one in the same chunk are still
 * returned, and every later push reports the same error.
 */
export interface FrameBatch {
  envelopes: ProtocolEnvelope[];
  error: FrameError | null;
}

export class FrameDecoder {
  private buffer = new Uint8Array(0);
  private failure: FrameError | null = null;
  private readonly text = new TextDecoder('utf-8', { fatal: true });

  push(chunk: Uint8Array): FrameBatch {
    if (this.failure) return { envelopes: [], error: this.failure };
    const joined = new Uint8Array(this.buffer.length + chunk.length);
    joined.set(this.buffer, 0);
    joined.set(chunk, this.buffer.length);
    this.buffer = joined;

    const envelopes: ProtocolEnvelope[] = [];
    while (this.buffer.length >= 4) {
      const length = new DataView(this.buffer.buffer, this.buffer.byteOffset, 4).getUint32(0, false);
      if (length > MAX_FRAME_SIZE_BYTES) {
        return this.fail(envelopes, `incoming frame of ${length} bytes exceeds the ${MAX_FRAME_SIZE_BYTES} byte limit`);
      }
      if (this.buffer.length < 4 + length) break;
      const body = this.buffer.subarray(4, 4 + length);
      let envelope: ProtocolEnvelope;
      try {
        envelope = envelopeFromJson(this.text.decode(body));
      } catch (error) {
        const message = error instanceof FrameError ? error.message : 'frame body is not valid UTF-8';
        return this.fail(envelopes, message);
      }
      envelopes.push(envelope);
      this.buffer = this.buffer.slice(4 + length);
    }
    return { envelopes, error: null };
  }

  get poisoned(): boolean {
    return this.failure !== null;
  }

  private fail(envelopes: ProtocolEnvelope[], message: string): FrameBatch {
    this.failure = new FrameError(message);
    this.buffer = new Uint8Array(0);
    return { envelopes, error: this.failure };
  }
}
