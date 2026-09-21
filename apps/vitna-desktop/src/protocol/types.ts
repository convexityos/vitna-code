/**
 * The Vitna Agent Protocol, version 1, as DECLARED in
 * protocol/vitna/protocol/v1/{envelope,commands,events}.proto.
 *
 * These are the declared messages, not what the Rust crates emit today. No
 * crate compiles the .proto files, and the orchestration engine logs its own
 * ad-hoc names instead ("vitna.v1.TurnStarted", an ApprovalRequested carrying
 * no approval_id). The declared contract is the only one both sides of the
 * wire can be held to, so this frontend is written against it, and a daemon
 * that disagrees shows up here as an undecodable event rather than as a
 * silently different screen.
 *
 * Field names are the proto field names. The Rust side derives serde with the
 * same snake_case spelling, and a proto3 JSON encoder can be told to keep it.
 * Every uint64 in the protocol is a sequence number, a millisecond duration,
 * a token count or a byte budget, all far inside 2^53, so they are numbers
 * here and the codec refuses anything past Number.MAX_SAFE_INTEGER.
 */

export const PROTOCOL_VERSION_MAJOR = 1;
export const PROTOCOL_VERSION_MINOR = 0;
export const SCHEMA_VERSION = 1;
/** ADR-0005 admission step 1: a larger frame drops the connection. */
export const MAX_FRAME_SIZE_BYTES = 16 * 1024 * 1024;
export const TYPE_URL_PREFIX = 'type.vitna.ai/vitna.protocol.v1.';

// ---- envelope.proto --------------------------------------------------------

export interface ProtocolEnvelope {
  protocol_version_major: number;
  protocol_version_minor: number;
  schema_version: number;
  type_url: string;
  session_id: string;
  run_id: string;
  sequence: number;
  idempotency_key: string;
  payload: Uint8Array;
}

export interface HandshakeRequest {
  min_supported_version: number;
  max_supported_version: number;
  client_identifier: string;
}

export interface HandshakeResponse {
  selected_version_major: number;
  selected_version_minor: number;
  daemon_build_commit: string;
  accepted: boolean;
  rejection_reason: string;
}

/** In declaration order, so an encoder that writes the enum as its number decodes too. */
export const ERROR_CODES = [
  'ERROR_CODE_UNSPECIFIED',
  'ERROR_CODE_UNAUTHORIZED',
  'ERROR_CODE_INVALID_FRAME',
  'ERROR_CODE_UNSUPPORTED_VERSION',
  'ERROR_CODE_UNKNOWN_COMMAND',
  'ERROR_CODE_POLICY_DENIED',
  'ERROR_CODE_NEEDS_RECONCILIATION',
  'ERROR_CODE_TIMEOUT',
  'ERROR_CODE_INTERNAL',
] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

export interface ErrorResponse {
  code: ErrorCode;
  message: string;
  error_details: string;
  action_id: string;
}

// ---- commands.proto --------------------------------------------------------

export const SESSION_MODES = ['inspect', 'build', 'autopilot'] as const;
export type SessionMode = (typeof SESSION_MODES)[number];

export interface CreateSession {
  workspace_path: string;
  title: string;
  initial_mode: SessionMode;
  idempotency_key: string;
}

export interface SubmitTurn {
  session_id: string;
  prompt: string;
  attached_file_paths: string[];
  idempotency_key: string;
}

export interface SteerRun {
  run_id: string;
  feedback: string;
  idempotency_key: string;
}

export interface PauseRun {
  run_id: string;
  reason: string;
}

export interface ResumeRun {
  run_id: string;
}

export interface CancelRun {
  run_id: string;
  reason: string;
}

export const APPROVAL_SCOPES = ['once', 'session', 'project'] as const;
export type ApprovalScope = (typeof APPROVAL_SCOPES)[number];

export interface ApproveAction {
  approval_id: string;
  action_digest: string;
  scope: ApprovalScope;
}

export interface RejectAction {
  approval_id: string;
  action_digest: string;
  reason: string;
}

export interface SpawnChild {
  parent_run_id: string;
  goal: string;
  requested_mode: string;
  token_budget: number;
  max_nesting_depth: number;
  idempotency_key: string;
}

export interface ApplyChangeSet {
  change_set_id: string;
  target_checkout_path: string;
  expected_base_commit: string;
  idempotency_key: string;
}

export interface AttachTerminal {
  session_id: string;
  terminal_columns: number;
  terminal_rows: number;
}

export interface SubscribeEvents {
  session_id: string;
  resume_after_sequence: number;
}

/** Everything a client sends. HandshakeRequest is declared in envelope.proto but travels the same way. */
export interface CommandBodies {
  HandshakeRequest: HandshakeRequest;
  CreateSession: CreateSession;
  SubmitTurn: SubmitTurn;
  SteerRun: SteerRun;
  PauseRun: PauseRun;
  ResumeRun: ResumeRun;
  CancelRun: CancelRun;
  ApproveAction: ApproveAction;
  RejectAction: RejectAction;
  SpawnChild: SpawnChild;
  ApplyChangeSet: ApplyChangeSet;
  AttachTerminal: AttachTerminal;
  SubscribeEvents: SubscribeEvents;
}
export type CommandKind = keyof CommandBodies;
export type ClientCommand = {
  [K in CommandKind]: { kind: K; body: CommandBodies[K] };
}[CommandKind];

// ---- events.proto ----------------------------------------------------------

export interface MessageDelta {
  message_id: string;
  /** "model", "system" or "user" per the proto comment; kept open so a new role renders rather than vanishes. */
  role: string;
  delta_text: string;
}

export interface ReasoningSummaryDelta {
  step_id: string;
  delta_text: string;
}

export interface PlanChanged {
  plan_id: string;
  steps: string[];
  active_step_index: number;
}

export const EFFECT_CLASSES = ['read', 'write', 'execute', 'network', 'secret'] as const;
export type EffectClass = (typeof EFFECT_CLASSES)[number];

export interface ToolProposed {
  tool_call_id: string;
  tool_name: string;
  tool_version: string;
  definition_digest: string;
  argument_digest: string;
  effect_class: string;
  raw_arguments_json: Uint8Array;
}

export interface ApprovalRequested {
  approval_id: string;
  tool_call_id: string;
  action_digest: string;
  description: string;
  executable_identity: string;
  canonical_cwd: string;
  environment_names: string[];
  bound_mounts: string[];
  /** Zero is proto3's unset, which this client reads as "no deadline given". */
  timeout_ms: number;
}

export interface ToolStarted {
  tool_call_id: string;
  started_at_ms: number;
}

export interface ToolOutput {
  tool_call_id: string;
  /** "stdout" or "stderr". */
  stream: string;
  chunk: Uint8Array;
}

export interface ToolFinished {
  tool_call_id: string;
  exit_code: number;
  stdout_digest: string;
  stderr_digest: string;
  duration_ms: number;
  /** "completed", "failed" or "cancelled". */
  status: string;
}

export interface DiffChanged {
  agent_workspace_id: string;
  diff_text: string;
  diff_digest: string;
  modified_files: string[];
}

export interface AgentSpawned {
  child_run_id: string;
  parent_run_id: string;
  goal: string;
  agent_workspace_path: string;
}

export interface AgentCompleted {
  child_run_id: string;
  completion_state: string;
  result_artifact_digest: string;
}

export interface UsageUpdated {
  run_id: string;
  provider: string;
  model_sku: string;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  /** When false, cost_usd means nothing and is never printed. */
  cost_known: boolean;
  cost_usd: number;
}

export const RUN_STATES = [
  'queued',
  'running_model',
  'waiting_for_approval',
  'running_tool',
  'waiting_for_child',
  'waiting_for_retry',
  'paused',
  'needs_reconciliation',
  'completed',
  'failed',
  'cancelled',
  'orphaned',
] as const;
export type RunState = (typeof RUN_STATES)[number];

export const TERMINAL_RUN_STATES: ReadonlySet<string> = new Set([
  'completed',
  'failed',
  'cancelled',
  'orphaned',
]);

export interface RunStateChanged {
  run_id: string;
  previous_state: string;
  new_state: string;
  reason: string;
}

export interface Diagnostic {
  /** "info", "warn" or "error". */
  level: string;
  code: string;
  message: string;
  details: string;
}

/** Everything the daemon sends. The two responses from envelope.proto share the stream. */
export interface EventBodies {
  HandshakeResponse: HandshakeResponse;
  ErrorResponse: ErrorResponse;
  MessageDelta: MessageDelta;
  ReasoningSummaryDelta: ReasoningSummaryDelta;
  PlanChanged: PlanChanged;
  ToolProposed: ToolProposed;
  ApprovalRequested: ApprovalRequested;
  ToolStarted: ToolStarted;
  ToolOutput: ToolOutput;
  ToolFinished: ToolFinished;
  DiffChanged: DiffChanged;
  AgentSpawned: AgentSpawned;
  AgentCompleted: AgentCompleted;
  UsageUpdated: UsageUpdated;
  RunStateChanged: RunStateChanged;
  Diagnostic: Diagnostic;
}
export type EventKind = keyof EventBodies;
export type DaemonEvent = {
  [K in EventKind]: { kind: K; body: EventBodies[K] };
}[EventKind];

export function typeUrlFor(kind: CommandKind | EventKind): string {
  return TYPE_URL_PREFIX + kind;
}
