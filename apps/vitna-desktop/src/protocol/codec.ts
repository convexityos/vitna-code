/**
 * Payload codec: the bytes inside an envelope, to and from typed messages.
 *
 * A payload is UTF-8 JSON with the proto field names. Two JSON dialects can
 * produce it and both are accepted on the way in:
 *
 * - serde, which is what the Rust crates derive: bytes as an array of
 *   numbers, uint64 as a number, enums as whatever the Rust type says.
 * - proto3 JSON: bytes as base64, uint64 as a decimal string, enums by name.
 *
 * proto3 omits a field that holds its default, so a MISSING field decodes to
 * that default (empty string, zero, false, empty list). A field that is
 * PRESENT with the wrong type is an error, never a guess: a screen that
 * quietly shows an empty digest because the daemon sent a number is the
 * failure this layer exists to prevent.
 */

import {
  ERROR_CODES,
  TYPE_URL_PREFIX,
  PROTOCOL_VERSION_MAJOR,
  PROTOCOL_VERSION_MINOR,
  SCHEMA_VERSION,
  type ClientCommand,
  type DaemonEvent,
  type ErrorCode,
  type EventBodies,
  type EventKind,
  type ProtocolEnvelope,
} from './types';

export class DecodeError extends Error {
  override readonly name = 'DecodeError';
}

type Obj = Record<string, unknown>;

const utf8Encoder = new TextEncoder();

function asObject(value: unknown, where: string): Obj {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new DecodeError(`${where}: expected an object`);
  }
  return value as Obj;
}

function str(o: Obj, key: string): string {
  const v = o[key];
  if (v === undefined || v === null) return '';
  if (typeof v !== 'string') throw new DecodeError(`${key}: expected a string`);
  return v;
}

function unsigned(o: Obj, key: string, max: number): number {
  const v = o[key];
  if (v === undefined || v === null) return 0;
  let n: number;
  if (typeof v === 'number') {
    n = v;
  } else if (typeof v === 'string' && /^\d+$/.test(v)) {
    // proto3 JSON writes uint64 as a decimal string.
    n = Number(v);
  } else {
    throw new DecodeError(`${key}: expected an unsigned integer`);
  }
  if (!Number.isInteger(n) || n < 0 || n > max) {
    throw new DecodeError(`${key}: ${String(v)} is out of range`);
  }
  return n;
}

const u64 = (o: Obj, key: string) => unsigned(o, key, Number.MAX_SAFE_INTEGER);
const u32 = (o: Obj, key: string) => unsigned(o, key, 0xffff_ffff);

function i32(o: Obj, key: string): number {
  const v = o[key];
  if (v === undefined || v === null) return 0;
  const n = typeof v === 'string' && /^-?\d+$/.test(v) ? Number(v) : v;
  if (typeof n !== 'number' || !Number.isInteger(n) || n < -0x8000_0000 || n > 0x7fff_ffff) {
    throw new DecodeError(`${key}: expected a 32-bit integer`);
  }
  return n;
}

function double(o: Obj, key: string): number {
  const v = o[key];
  if (v === undefined || v === null) return 0;
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new DecodeError(`${key}: expected a finite number`);
  }
  return v;
}

function bool(o: Obj, key: string): boolean {
  const v = o[key];
  if (v === undefined || v === null) return false;
  if (typeof v !== 'boolean') throw new DecodeError(`${key}: expected a boolean`);
  return v;
}

function strList(o: Obj, key: string): string[] {
  const v = o[key];
  if (v === undefined || v === null) return [];
  if (!Array.isArray(v) || v.some((item) => typeof item !== 'string')) {
    throw new DecodeError(`${key}: expected a list of strings`);
  }
  return v as string[];
}

export function base64ToBytes(text: string): Uint8Array {
  if (!/^[A-Za-z0-9+/_-]*={0,2}$/.test(text)) throw new DecodeError('invalid base64');
  const normal = text.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(normal);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

export function bytesFromJson(value: unknown, key: string): Uint8Array {
  if (value === undefined || value === null) return new Uint8Array(0);
  if (typeof value === 'string') {
    try {
      return base64ToBytes(value);
    } catch {
      throw new DecodeError(`${key}: invalid base64`);
    }
  }
  if (Array.isArray(value)) {
    const out = new Uint8Array(value.length);
    for (let i = 0; i < value.length; i += 1) {
      const b = value[i];
      if (typeof b !== 'number' || !Number.isInteger(b) || b < 0 || b > 255) {
        throw new DecodeError(`${key}: byte ${i} is not 0-255`);
      }
      out[i] = b;
    }
    return out;
  }
  throw new DecodeError(`${key}: expected bytes`);
}

const bytes = (o: Obj, key: string) => bytesFromJson(o[key], key);

function errorCode(o: Obj, key: string): ErrorCode {
  const v = o[key];
  if (v === undefined || v === null) return 'ERROR_CODE_UNSPECIFIED';
  if (typeof v === 'number') {
    const name = ERROR_CODES[v];
    if (name === undefined) throw new DecodeError(`${key}: unknown error code ${v}`);
    return name;
  }
  if (typeof v === 'string' && (ERROR_CODES as readonly string[]).includes(v)) {
    return v as ErrorCode;
  }
  throw new DecodeError(`${key}: unknown error code ${String(v)}`);
}

type Decoders = { [K in EventKind]: (o: Obj) => EventBodies[K] };

const EVENT_DECODERS: Decoders = {
  HandshakeResponse: (o) => ({
    selected_version_major: u32(o, 'selected_version_major'),
    selected_version_minor: u32(o, 'selected_version_minor'),
    daemon_build_commit: str(o, 'daemon_build_commit'),
    accepted: bool(o, 'accepted'),
    rejection_reason: str(o, 'rejection_reason'),
  }),
  ErrorResponse: (o) => ({
    code: errorCode(o, 'code'),
    message: str(o, 'message'),
    error_details: str(o, 'error_details'),
    action_id: str(o, 'action_id'),
  }),
  MessageDelta: (o) => ({
    message_id: str(o, 'message_id'),
    role: str(o, 'role'),
    delta_text: str(o, 'delta_text'),
  }),
  ReasoningSummaryDelta: (o) => ({
    step_id: str(o, 'step_id'),
    delta_text: str(o, 'delta_text'),
  }),
  PlanChanged: (o) => ({
    plan_id: str(o, 'plan_id'),
    steps: strList(o, 'steps'),
    active_step_index: u32(o, 'active_step_index'),
  }),
  ToolProposed: (o) => ({
    tool_call_id: str(o, 'tool_call_id'),
    tool_name: str(o, 'tool_name'),
    tool_version: str(o, 'tool_version'),
    definition_digest: str(o, 'definition_digest'),
    argument_digest: str(o, 'argument_digest'),
    effect_class: str(o, 'effect_class'),
    raw_arguments_json: bytes(o, 'raw_arguments_json'),
  }),
  ApprovalRequested: (o) => ({
    approval_id: str(o, 'approval_id'),
    tool_call_id: str(o, 'tool_call_id'),
    action_digest: str(o, 'action_digest'),
    description: str(o, 'description'),
    executable_identity: str(o, 'executable_identity'),
    canonical_cwd: str(o, 'canonical_cwd'),
    environment_names: strList(o, 'environment_names'),
    bound_mounts: strList(o, 'bound_mounts'),
    timeout_ms: u64(o, 'timeout_ms'),
  }),
  ToolStarted: (o) => ({
    tool_call_id: str(o, 'tool_call_id'),
    started_at_ms: u64(o, 'started_at_ms'),
  }),
  ToolOutput: (o) => ({
    tool_call_id: str(o, 'tool_call_id'),
    stream: str(o, 'stream'),
    chunk: bytes(o, 'chunk'),
  }),
  ToolFinished: (o) => ({
    tool_call_id: str(o, 'tool_call_id'),
    exit_code: i32(o, 'exit_code'),
    stdout_digest: str(o, 'stdout_digest'),
    stderr_digest: str(o, 'stderr_digest'),
    duration_ms: u64(o, 'duration_ms'),
    status: str(o, 'status'),
  }),
  DiffChanged: (o) => ({
    agent_workspace_id: str(o, 'agent_workspace_id'),
    diff_text: str(o, 'diff_text'),
    diff_digest: str(o, 'diff_digest'),
    modified_files: strList(o, 'modified_files'),
  }),
  AgentSpawned: (o) => ({
    child_run_id: str(o, 'child_run_id'),
    parent_run_id: str(o, 'parent_run_id'),
    goal: str(o, 'goal'),
    agent_workspace_path: str(o, 'agent_workspace_path'),
  }),
  AgentCompleted: (o) => ({
    child_run_id: str(o, 'child_run_id'),
    completion_state: str(o, 'completion_state'),
    result_artifact_digest: str(o, 'result_artifact_digest'),
  }),
  UsageUpdated: (o) => ({
    run_id: str(o, 'run_id'),
    provider: str(o, 'provider'),
    model_sku: str(o, 'model_sku'),
    prompt_tokens: u64(o, 'prompt_tokens'),
    completion_tokens: u64(o, 'completion_tokens'),
    cached_tokens: u64(o, 'cached_tokens'),
    cost_known: bool(o, 'cost_known'),
    cost_usd: double(o, 'cost_usd'),
  }),
  RunStateChanged: (o) => ({
    run_id: str(o, 'run_id'),
    previous_state: str(o, 'previous_state'),
    new_state: str(o, 'new_state'),
    reason: str(o, 'reason'),
  }),
  Diagnostic: (o) => ({
    level: str(o, 'level'),
    code: str(o, 'code'),
    message: str(o, 'message'),
    details: str(o, 'details'),
  }),
};

export function isEventKind(name: string): name is EventKind {
  return Object.prototype.hasOwnProperty.call(EVENT_DECODERS, name);
}

/** The message name a type URL carries, or null when it is not a v1 URL at all. */
export function kindFromTypeUrl(typeUrl: string): string | null {
  return typeUrl.startsWith(TYPE_URL_PREFIX) ? typeUrl.slice(TYPE_URL_PREFIX.length) : null;
}

export type DecodedEnvelope =
  | { ok: true; event: DaemonEvent }
  | { ok: false; reason: 'unknown_type' | 'unsupported_version' | 'malformed'; detail: string };

/**
 * Decodes one envelope from the daemon. Never throws: an envelope this client
 * cannot read comes back as a reason, so the screen can say that an event was
 * not understood instead of dropping it (the Honesty Contract's "unknown stays
 * unknown").
 */
export function decodeEnvelope(envelope: ProtocolEnvelope): DecodedEnvelope {
  if (envelope.protocol_version_major !== PROTOCOL_VERSION_MAJOR) {
    return {
      ok: false,
      reason: 'unsupported_version',
      detail: `protocol ${envelope.protocol_version_major}.${envelope.protocol_version_minor}`,
    };
  }
  const name = kindFromTypeUrl(envelope.type_url);
  if (name === null || !isEventKind(name)) {
    return { ok: false, reason: 'unknown_type', detail: envelope.type_url };
  }
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(envelope.payload);
    const parsed: unknown = text.length === 0 ? {} : JSON.parse(text);
    const body = EVENT_DECODERS[name](asObject(parsed, name));
    return { ok: true, event: { kind: name, body } as DaemonEvent };
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    return { ok: false, reason: 'malformed', detail: `${name}: ${detail}` };
  }
}

export interface EnvelopeContext {
  session_id: string;
  run_id: string;
  sequence: number;
  idempotency_key: string;
}

/** Wraps a command in an envelope. Commands carry no bytes fields, so plain JSON is exact. */
export function encodeCommand(command: ClientCommand, context: EnvelopeContext): ProtocolEnvelope {
  return {
    protocol_version_major: PROTOCOL_VERSION_MAJOR,
    protocol_version_minor: PROTOCOL_VERSION_MINOR,
    schema_version: SCHEMA_VERSION,
    type_url: TYPE_URL_PREFIX + command.kind,
    session_id: context.session_id,
    run_id: context.run_id,
    sequence: context.sequence,
    idempotency_key: context.idempotency_key,
    payload: utf8Encoder.encode(JSON.stringify(command.body)),
  };
}

/**
 * The inverse of decodeEnvelope, for the one place that has to speak as the
 * daemon: the DEV-only sample transport. Byte fields are written the serde
 * way, as arrays of numbers.
 */
export function encodeEvent(event: DaemonEvent, context: EnvelopeContext): ProtocolEnvelope {
  const body = JSON.stringify(event.body, (_key, value: unknown) =>
    value instanceof Uint8Array ? Array.from(value) : value,
  );
  return {
    protocol_version_major: PROTOCOL_VERSION_MAJOR,
    protocol_version_minor: PROTOCOL_VERSION_MINOR,
    schema_version: SCHEMA_VERSION,
    type_url: TYPE_URL_PREFIX + event.kind,
    session_id: context.session_id,
    run_id: context.run_id,
    sequence: context.sequence,
    idempotency_key: context.idempotency_key,
    payload: utf8Encoder.encode(body),
  };
}

/** Reads a command back out of an envelope, for the sample transport's side of the wire. */
export function decodeCommandPayload(envelope: ProtocolEnvelope): { kind: string; body: Obj } {
  const kind = kindFromTypeUrl(envelope.type_url) ?? envelope.type_url;
  const text = new TextDecoder('utf-8', { fatal: true }).decode(envelope.payload);
  return { kind, body: asObject(text.length === 0 ? {} : JSON.parse(text), kind) };
}
