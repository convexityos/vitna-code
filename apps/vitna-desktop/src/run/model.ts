/**
 * What one session looks like after its event stream has been folded, and the
 * actions that fold it. Everything here is data; src/run/reducer.ts is the
 * only thing that produces it.
 */

import type {
  AgentCompleted,
  AgentSpawned,
  ApprovalRequested,
  ApprovalScope,
  DaemonEvent,
  DiffChanged,
  PlanChanged,
  ToolFinished,
  ToolProposed,
  UsageUpdated,
} from '../protocol/types';

/** Output a single tool call may hold in this window before the rest is counted, not kept. */
export const MAX_TOOL_OUTPUT_BYTES = 256 * 1024;

export interface EventMeta {
  sequence: number;
  session_id: string;
  run_id: string;
  type_url: string;
}

export interface OutputSegment {
  stream: string;
  chunks: Uint8Array[];
  bytes: number;
}

export interface ToolCallView {
  id: string;
  runId: string;
  proposed: ToolProposed | null;
  startedAtMs: number | null;
  segments: OutputSegment[];
  keptBytes: number;
  droppedBytes: number;
  finished: ToolFinished | null;
  approvalIds: string[];
}

/**
 * Where an approval stands. The first four are this window's side of the
 * exchange; the last four are what the daemon's own events showed.
 */
export type ApprovalStatus =
  | 'pending'
  | 'sending'
  | 'sent'
  | 'send_failed'
  | 'refused'
  | 'approved'
  | 'rejected'
  | 'ended';

export const OPEN_APPROVAL_STATUSES: ReadonlySet<ApprovalStatus> = new Set([
  'pending',
  'sending',
  'sent',
  'send_failed',
]);

export type ApprovalChoice = 'approve' | 'reject';

export interface ApprovalView {
  request: ApprovalRequested;
  runId: string;
  sequence: number;
  /** Client clock when the request arrived. Used for the countdown and nothing else. */
  receivedAt: number;
  status: ApprovalStatus;
  choice: ApprovalChoice | null;
  scope: ApprovalScope | null;
  reason: string;
  /** The daemon's refusal, or the transport's failure, in its own words. */
  message: string;
}

export interface MessageView {
  id: string;
  role: string;
  text: string;
}

export interface ReasoningView {
  id: string;
  text: string;
}

export interface ChildView {
  id: string;
  spawned: AgentSpawned | null;
  completed: AgentCompleted | null;
}

export interface DiagnosticView {
  id: string;
  origin: 'daemon' | 'client';
  level: string;
  code: string;
  message: string;
  details: string;
}

export interface UnknownView {
  id: string;
  typeUrl: string;
  reason: string;
  detail: string;
}

export interface RunSummary {
  id: string;
  state: string | null;
  reason: string;
  /** Client clock when this window first saw the run. For an elapsed counter only. */
  firstSeenAt: number;
  /** Client clock of the latest state change. */
  changedAt: number;
}

export interface TurnView {
  id: string;
  prompt: string;
  sentAt: number;
}

export type TimelineItem =
  | { type: 'run'; id: string }
  | { type: 'plan'; id: string }
  | { type: 'turn'; id: string }
  | { type: 'message'; id: string }
  | { type: 'reasoning'; id: string }
  | { type: 'tool'; id: string }
  | { type: 'approval'; id: string }
  | { type: 'child'; id: string }
  | { type: 'diagnostic'; id: string }
  | { type: 'unknown'; id: string };

export interface SequenceGap {
  after: number;
  received: number;
}

export interface SessionView {
  sessionId: string;
  lastSequence: number | null;
  gap: SequenceGap | null;
  currentRunId: string | null;
  runs: Record<string, RunSummary>;
  plan: PlanChanged | null;
  timeline: TimelineItem[];
  turns: Record<string, TurnView>;
  messages: Record<string, MessageView>;
  reasoning: Record<string, ReasoningView>;
  tools: Record<string, ToolCallView>;
  approvals: Record<string, ApprovalView>;
  diffs: Record<string, DiffChanged>;
  diffOrder: string[];
  children: Record<string, ChildView>;
  usage: Record<string, UsageUpdated>;
  diagnostics: Record<string, DiagnosticView>;
  unknown: Record<string, UnknownView>;
}

export type SessionAction =
  | { type: 'event'; meta: EventMeta; event: DaemonEvent; receivedAt: number }
  | { type: 'undecodable'; meta: EventMeta; reason: string; detail: string }
  | { type: 'turn/sent'; id: string; prompt: string; sentAt: number }
  | {
      type: 'approval/sending';
      approvalId: string;
      choice: ApprovalChoice;
      scope: ApprovalScope | null;
      reason: string;
    }
  | { type: 'approval/sent'; approvalId: string }
  | { type: 'approval/send-failed'; approvalId: string; message: string }
  | { type: 'client/diagnostic'; level: string; code: string; message: string; details: string }
  | { type: 'gap/resubscribed' };

export function emptySession(sessionId: string): SessionView {
  return {
    sessionId,
    lastSequence: null,
    gap: null,
    currentRunId: null,
    runs: {},
    plan: null,
    timeline: [],
    turns: {},
    messages: {},
    reasoning: {},
    tools: {},
    approvals: {},
    diffs: {},
    diffOrder: [],
    children: {},
    usage: {},
    diagnostics: {},
    unknown: {},
  };
}
