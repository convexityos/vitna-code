/**
 * Folds a session's event stream into a SessionView.
 *
 * Pure: no clock, no randomness, no I/O. Time arrives on the action
 * (receivedAt, sentAt) so a replayed stream folds to the same view.
 *
 * Three rules carry the weight:
 *
 * 1. Sequence. ADR-0005 gives every daemon event a strictly monotonic
 *    sequence and lets a client resume after the last one it holds. So a
 *    sequence at or below the last applied one is a replay and is ignored,
 *    and a sequence that skips ahead is NOT applied: applying it would make
 *    the missing events look like replays when they arrive. The view records
 *    the gap, the session layer resubscribes from the last applied sequence,
 *    and the stream continues in order.
 *
 * 2. An approval decision is final once it leaves this window. A second
 *    decision for the same request is ignored rather than sent, so no key
 *    press can turn an approval into a rejection after the fact. Only a
 *    delivery failure reopens it.
 *
 * 3. Outcomes come from the daemon's events, never from the button that was
 *    pressed. "Approved" means the tool started. A tool that starts after this
 *    window rejected it is raised as an error, not smoothed over.
 */

import { TERMINAL_RUN_STATES, type DaemonEvent, type ErrorResponse } from '../protocol/types';
import {
  MAX_TOOL_OUTPUT_BYTES,
  OPEN_APPROVAL_STATUSES,
  type ApprovalView,
  type DiagnosticView,
  type EventMeta,
  type SessionAction,
  type SessionView,
  type TimelineItem,
  type ToolCallView,
} from './model';

const PROGRESS_STATES: ReadonlySet<string> = new Set(['running_model', 'running_tool', 'waiting_for_child', 'queued']);

export function sessionReducer(state: SessionView, action: SessionAction): SessionView {
  switch (action.type) {
    case 'event':
      return applyEvent(state, action.meta, action.event, action.receivedAt);
    case 'undecodable':
      return admit(state, action.meta, (s) =>
        withTimeline(
          {
            ...s,
            unknown: {
              ...s.unknown,
              [`unknown@${action.meta.sequence}`]: {
                id: `unknown@${action.meta.sequence}`,
                typeUrl: action.meta.type_url,
                reason: action.reason,
                detail: action.detail,
              },
            },
          },
          { type: 'unknown', id: `unknown@${action.meta.sequence}` },
        ),
      );
    case 'turn/sent':
      if (state.turns[action.id]) return state;
      return withTimeline(
        {
          ...state,
          turns: {
            ...state.turns,
            [action.id]: { id: action.id, prompt: action.prompt, sentAt: action.sentAt },
          },
        },
        { type: 'turn', id: action.id },
      );
    case 'approval/sending': {
      const approval = state.approvals[action.approvalId];
      if (!approval) return state;
      if (approval.status !== 'pending' && approval.status !== 'send_failed') return state;
      return putApproval(state, {
        ...approval,
        status: 'sending',
        choice: action.choice,
        scope: action.choice === 'approve' ? action.scope : null,
        reason: action.reason,
        message: '',
      });
    }
    case 'approval/sent': {
      const approval = state.approvals[action.approvalId];
      if (!approval || approval.status !== 'sending') return state;
      return putApproval(state, { ...approval, status: 'sent' });
    }
    case 'approval/send-failed': {
      const approval = state.approvals[action.approvalId];
      if (!approval || approval.status !== 'sending') return state;
      return putApproval(state, { ...approval, status: 'send_failed', message: action.message });
    }
    case 'client/diagnostic':
      return addDiagnostic(state, `client@${Object.keys(state.diagnostics).length}`, {
        origin: 'client',
        level: action.level,
        code: action.code,
        message: action.message,
        details: action.details,
      });
    case 'gap/resubscribed':
      return state;
  }
}

// ---- sequencing -------------------------------------------------------------

function admit(state: SessionView, meta: EventMeta, apply: (s: SessionView) => SessionView): SessionView {
  if (meta.session_id && meta.session_id !== state.sessionId) return state;
  const last = state.lastSequence;
  if (last !== null && meta.sequence <= last) return state;
  if (last !== null && meta.sequence > last + 1) {
    if (state.gap && state.gap.received <= meta.sequence) return state;
    return { ...state, gap: { after: last, received: meta.sequence } };
  }
  const next = apply(state);
  const runId = meta.run_id || next.currentRunId;
  return { ...next, lastSequence: meta.sequence, gap: null, currentRunId: runId };
}

// ---- events -----------------------------------------------------------------

function applyEvent(state: SessionView, meta: EventMeta, event: DaemonEvent, receivedAt: number): SessionView {
  // A handshake is the transport's business and never reaches a session.
  if (event.kind === 'HandshakeResponse') return state;
  // Error responses answer a command rather than extend the run, so they sit
  // outside the event sequence.
  if (event.kind === 'ErrorResponse') return applyError(state, event.body);

  return admit(state, meta, (s) => {
    const at = meta.sequence;
    const runId = meta.run_id;
    switch (event.kind) {
      case 'RunStateChanged': {
        const b = event.body;
        const id = b.run_id || runId;
        const known = s.runs[id];
        let next: SessionView = {
          ...s,
          runs: {
            ...s.runs,
            [id]: {
              id,
              state: b.new_state,
              reason: b.reason,
              firstSeenAt: known?.firstSeenAt ?? receivedAt,
              changedAt: receivedAt,
            },
          },
          currentRunId: id,
        };
        if (!known) next = withTimeline(next, { type: 'run', id });
        if (known && b.previous_state && known.state && known.state !== b.previous_state) {
          next = addDiagnostic(next, `client@state@${at}`, {
            origin: 'client',
            level: 'warn',
            code: 'state_mismatch',
            message: `The daemon moved this run from "${b.previous_state}", but this window last saw "${known.state}".`,
            details: '',
          });
        }
        if (TERMINAL_RUN_STATES.has(b.new_state)) {
          next = closeApprovals(next, id, 'ended');
        } else if (b.previous_state === 'waiting_for_approval' && PROGRESS_STATES.has(b.new_state)) {
          // Pausing while a request is open is not an answer to it; moving on is.
          next = settleRejections(next, id);
        }
        return next;
      }
      case 'MessageDelta': {
        const b = event.body;
        const id = b.message_id || `message@${at}`;
        const existing = s.messages[id];
        const next = {
          ...s,
          messages: {
            ...s.messages,
            [id]: { id, role: existing?.role || b.role, text: (existing?.text ?? '') + b.delta_text },
          },
        };
        return existing ? next : withTimeline(next, { type: 'message', id });
      }
      case 'ReasoningSummaryDelta': {
        const b = event.body;
        const id = b.step_id || `reasoning@${at}`;
        const existing = s.reasoning[id];
        const next = {
          ...s,
          reasoning: { ...s.reasoning, [id]: { id, text: (existing?.text ?? '') + b.delta_text } },
        };
        return existing ? next : withTimeline(next, { type: 'reasoning', id });
      }
      case 'PlanChanged': {
        // The plan sits in the transcript where it first appeared and updates in place.
        const id = event.body.plan_id || 'plan';
        const next = { ...s, plan: { ...event.body, plan_id: id } };
        const placed = s.timeline.some((item) => item.type === 'plan' && item.id === id);
        return placed ? next : withTimeline(next, { type: 'plan', id });
      }
      case 'ToolProposed': {
        const b = event.body;
        const id = b.tool_call_id || `tool@${at}`;
        const { next, tool } = ensureTool(s, id, runId);
        return putTool(next, { ...tool, proposed: b });
      }
      case 'ApprovalRequested': {
        const b = event.body;
        const id = b.approval_id || `approval@${at}`;
        if (s.approvals[id]) return s;
        const approval: ApprovalView = {
          request: { ...b, approval_id: id },
          runId,
          sequence: at,
          receivedAt,
          status: 'pending',
          choice: null,
          scope: null,
          reason: '',
          message: '',
        };
        let next: SessionView = { ...s, approvals: { ...s.approvals, [id]: approval } };
        const tool = b.tool_call_id ? next.tools[b.tool_call_id] : undefined;
        if (tool) {
          next = putTool(next, { ...tool, approvalIds: [...tool.approvalIds, id] });
        } else {
          // Nothing proposed this action on the stream, so it gets its own place.
          next = withTimeline(next, { type: 'approval', id });
        }
        return next;
      }
      case 'ToolStarted': {
        const b = event.body;
        const { next, tool } = ensureTool(s, b.tool_call_id || `tool@${at}`, runId);
        let out = putTool(next, { ...tool, startedAtMs: b.started_at_ms });
        for (const approvalId of tool.approvalIds) {
          const approval = out.approvals[approvalId];
          if (!approval || !OPEN_APPROVAL_STATUSES.has(approval.status)) continue;
          if (approval.choice === 'reject') {
            out = addDiagnostic(out, `client@rejected-ran@${at}`, {
              origin: 'client',
              level: 'error',
              code: 'rejected_action_started',
              message: 'A tool this window rejected has started.',
              details: `approval ${approvalId}, tool call ${tool.id}`,
            });
          }
          out = putApproval(out, { ...approval, status: 'approved' });
        }
        return out;
      }
      case 'ToolOutput': {
        const b = event.body;
        const { next, tool } = ensureTool(s, b.tool_call_id || `tool@${at}`, runId);
        return putTool(next, appendOutput(tool, b.stream, b.chunk));
      }
      case 'ToolFinished': {
        const b = event.body;
        const { next, tool } = ensureTool(s, b.tool_call_id || `tool@${at}`, runId);
        let out = putTool(next, { ...tool, finished: b });
        if (tool.startedAtMs === null) {
          for (const approvalId of tool.approvalIds) {
            const approval = out.approvals[approvalId];
            if (!approval || !OPEN_APPROVAL_STATUSES.has(approval.status)) continue;
            out = putApproval(out, {
              ...approval,
              status: approval.choice === 'reject' ? 'rejected' : 'ended',
            });
          }
        }
        return out;
      }
      case 'DiffChanged': {
        const b = event.body;
        const id = b.agent_workspace_id || 'workspace';
        return {
          ...s,
          diffs: { ...s.diffs, [id]: b },
          diffOrder: s.diffOrder.includes(id) ? s.diffOrder : [...s.diffOrder, id],
        };
      }
      case 'AgentSpawned': {
        const b = event.body;
        const id = b.child_run_id || `child@${at}`;
        const existing = s.children[id];
        const next = {
          ...s,
          children: {
            ...s.children,
            [id]: { id, spawned: b, completed: existing?.completed ?? null },
          },
        };
        return existing ? next : withTimeline(next, { type: 'child', id });
      }
      case 'AgentCompleted': {
        const b = event.body;
        const id = b.child_run_id || `child@${at}`;
        const existing = s.children[id];
        const next = {
          ...s,
          children: {
            ...s.children,
            [id]: { id, spawned: existing?.spawned ?? null, completed: b },
          },
        };
        return existing ? next : withTimeline(next, { type: 'child', id });
      }
      case 'UsageUpdated': {
        const b = event.body;
        const id = b.run_id || runId;
        return { ...s, usage: { ...s.usage, [id]: b } };
      }
      case 'Diagnostic':
        return addDiagnostic(s, `daemon@${at}`, { origin: 'daemon', ...event.body });
    }
  });
}

function applyError(state: SessionView, error: ErrorResponse): SessionView {
  const approval = error.action_id ? state.approvals[error.action_id] : undefined;
  // A refusal answers a decision, so it attaches only to one this window sent.
  // Anything else stays a diagnostic and the request stays open.
  if (approval && (approval.status === 'sending' || approval.status === 'sent')) {
    return putApproval(state, {
      ...approval,
      status: 'refused',
      message: error.message || error.code,
    });
  }
  return addDiagnostic(state, `error@${Object.keys(state.diagnostics).length}`, {
    origin: 'daemon',
    level: 'error',
    code: error.code,
    message: error.message,
    details: [error.error_details, error.action_id && `action ${error.action_id}`]
      .filter(Boolean)
      .join(' / '),
  });
}

// ---- helpers ----------------------------------------------------------------

function withTimeline(state: SessionView, item: TimelineItem): SessionView {
  return { ...state, timeline: [...state.timeline, item] };
}

function putApproval(state: SessionView, approval: ApprovalView): SessionView {
  return { ...state, approvals: { ...state.approvals, [approval.request.approval_id]: approval } };
}

function putTool(state: SessionView, tool: ToolCallView): SessionView {
  return { ...state, tools: { ...state.tools, [tool.id]: tool } };
}

function ensureTool(state: SessionView, id: string, runId: string): { next: SessionView; tool: ToolCallView } {
  const existing = state.tools[id];
  if (existing) return { next: state, tool: existing };
  const tool: ToolCallView = {
    id,
    runId,
    proposed: null,
    startedAtMs: null,
    segments: [],
    keptBytes: 0,
    droppedBytes: 0,
    finished: null,
    approvalIds: [],
  };
  return { next: withTimeline(putTool(state, tool), { type: 'tool', id }), tool };
}

function appendOutput(tool: ToolCallView, stream: string, chunk: Uint8Array): ToolCallView {
  const room = Math.max(0, MAX_TOOL_OUTPUT_BYTES - tool.keptBytes);
  const kept = chunk.length <= room ? chunk : chunk.subarray(0, room);
  const dropped = chunk.length - kept.length;
  if (kept.length === 0) return { ...tool, droppedBytes: tool.droppedBytes + dropped };

  const segments = tool.segments.slice();
  const last = segments[segments.length - 1];
  if (last && last.stream === stream) {
    segments[segments.length - 1] = {
      stream,
      chunks: [...last.chunks, kept],
      bytes: last.bytes + kept.length,
    };
  } else {
    segments.push({ stream, chunks: [kept], bytes: kept.length });
  }
  return {
    ...tool,
    segments,
    keptBytes: tool.keptBytes + kept.length,
    droppedBytes: tool.droppedBytes + dropped,
  };
}

function addDiagnostic(state: SessionView, id: string, diagnostic: Omit<DiagnosticView, 'id'>): SessionView {
  let key = id;
  let n = 1;
  while (state.diagnostics[key]) key = `${id}#${n++}`;
  return withTimeline(
    { ...state, diagnostics: { ...state.diagnostics, [key]: { id: key, ...diagnostic } } },
    { type: 'diagnostic', id: key },
  );
}

function closeApprovals(state: SessionView, runId: string, status: 'ended'): SessionView {
  let out = state;
  for (const approval of Object.values(state.approvals)) {
    if (approval.runId !== runId || !OPEN_APPROVAL_STATUSES.has(approval.status)) continue;
    out = putApproval(out, {
      ...approval,
      status: approval.choice === 'reject' && approval.status === 'sent' ? 'rejected' : status,
    });
  }
  return out;
}

/** The run left waiting_for_approval without ending: a rejection this window sent has been taken. */
function settleRejections(state: SessionView, runId: string): SessionView {
  let out = state;
  for (const approval of Object.values(state.approvals)) {
    if (approval.runId !== runId || approval.status !== 'sent' || approval.choice !== 'reject') continue;
    const tool = out.tools[approval.request.tool_call_id];
    if (tool && tool.startedAtMs !== null) continue;
    out = putApproval(out, { ...approval, status: 'rejected' });
  }
  return out;
}
