import { describe, expect, it } from 'vitest';
import { TYPE_URL_PREFIX, type DaemonEvent } from '../protocol/types';
import { emptySession, MAX_TOOL_OUTPUT_BYTES, type SessionAction, type SessionView } from './model';
import { sessionReducer } from './reducer';

const DIGEST = 'f'.repeat(64);
const encode = (text: string) => new TextEncoder().encode(text);

function ev(sequence: number, event: DaemonEvent, meta: { run?: string; session?: string } = {}): SessionAction {
  return {
    type: 'event',
    meta: {
      sequence,
      session_id: meta.session ?? 'S',
      run_id: meta.run ?? 'R',
      type_url: TYPE_URL_PREFIX + event.kind,
    },
    event,
    receivedAt: 1_000,
  };
}

const fold = (actions: SessionAction[], start: SessionView = emptySession('S')) =>
  actions.reduce(sessionReducer, start);

const moved = (from: string, to: string, reason = ''): DaemonEvent => ({
  kind: 'RunStateChanged',
  body: { run_id: 'R', previous_state: from, new_state: to, reason },
});
const proposed = (id: string): DaemonEvent => ({
  kind: 'ToolProposed',
  body: {
    tool_call_id: id,
    tool_name: 'apply_patch',
    tool_version: '1.0.0',
    definition_digest: 'dd',
    argument_digest: 'ad',
    effect_class: 'write',
    raw_arguments_json: encode('{}'),
  },
});
const requested = (approvalId: string, toolId: string): DaemonEvent => ({
  kind: 'ApprovalRequested',
  body: {
    approval_id: approvalId,
    tool_call_id: toolId,
    action_digest: DIGEST,
    description: 'write a file',
    executable_identity: 'vitna-runner apply_patch',
    canonical_cwd: '/w',
    environment_names: [],
    bound_mounts: ['/w (read-write)'],
    timeout_ms: 0,
  },
});
const started = (id: string): DaemonEvent => ({ kind: 'ToolStarted', body: { tool_call_id: id, started_at_ms: 5 } });
const finished = (id: string, status = 'completed'): DaemonEvent => ({
  kind: 'ToolFinished',
  body: { tool_call_id: id, exit_code: 0, stdout_digest: '', stderr_digest: '', duration_ms: 12, status },
});
const said = (id: string, text: string): DaemonEvent => ({
  kind: 'MessageDelta',
  body: { message_id: id, role: 'model', delta_text: text },
});

/** A run paused on one approval: tool t1, approval a1. */
const waiting = () =>
  fold([
    ev(1, moved('', 'running_model')),
    ev(2, proposed('t1')),
    ev(3, moved('running_model', 'waiting_for_approval')),
    ev(4, requested('a1', 't1')),
  ]);

describe('sequencing', () => {
  it('ignores a replayed event', () => {
    const once = fold([ev(1, said('m', 'hello'))]);
    const twice = fold([ev(1, said('m', 'hello'))], once);
    expect(twice.messages.m?.text).toBe('hello');
    expect(twice).toBe(once);
  });

  it('holds back an event that skips ahead, and records the gap', () => {
    const view = fold([ev(1, said('m', 'a')), ev(3, said('m', 'c'))]);
    expect(view.messages.m?.text).toBe('a');
    expect(view.lastSequence).toBe(1);
    expect(view.gap).toEqual({ after: 1, received: 3 });
  });

  it('recovers to the same view once the missed events are resent', () => {
    const straight = fold([ev(1, said('m', 'a')), ev(2, said('m', 'b')), ev(3, said('m', 'c'))]);
    const recovered = fold([
      ev(1, said('m', 'a')),
      ev(3, said('m', 'c')),
      // the resubscription replays everything after 1
      ev(2, said('m', 'b')),
      ev(3, said('m', 'c')),
    ]);
    expect(recovered.gap).toBeNull();
    expect(recovered.messages).toEqual(straight.messages);
    expect(recovered.lastSequence).toBe(3);
  });

  it('ignores events addressed to another session', () => {
    const view = fold([ev(1, said('m', 'x'), { session: 'other' })]);
    expect(view.lastSequence).toBeNull();
    expect(view.messages).toEqual({});
  });

  it('surfaces an event it could not read, in order', () => {
    const view = fold([
      ev(1, said('m', 'a')),
      {
        type: 'undecodable',
        meta: { sequence: 2, session_id: 'S', run_id: 'R', type_url: 'vitna.v1.TurnStarted' },
        reason: 'unknown_type',
        detail: 'vitna.v1.TurnStarted',
      },
    ]);
    expect(view.timeline.map((item) => item.type)).toEqual(['message', 'unknown']);
    expect(Object.values(view.unknown)[0]?.typeUrl).toBe('vitna.v1.TurnStarted');
    expect(view.lastSequence).toBe(2);
  });
});

describe('approvals', () => {
  it('nests a request under the tool that proposed it', () => {
    const view = waiting();
    expect(view.tools.t1?.approvalIds).toEqual(['a1']);
    expect(view.timeline.some((item) => item.type === 'approval')).toBe(false);
    expect(view.approvals.a1?.status).toBe('pending');
    expect(view.runs.R?.state).toBe('waiting_for_approval');
  });

  it('gives an orphan request its own place in the timeline', () => {
    const view = fold([ev(1, requested('a9', 'never-proposed'))]);
    expect(view.timeline).toEqual([{ type: 'approval', id: 'a9' }]);
  });

  it('calls it approved only when the tool starts', () => {
    let view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'approve', scope: 'once', reason: '' },
        { type: 'approval/sent', approvalId: 'a1' },
      ],
      waiting(),
    );
    expect(view.approvals.a1?.status).toBe('sent');
    view = fold([ev(5, moved('waiting_for_approval', 'running_tool'))], view);
    expect(view.approvals.a1?.status).toBe('sent');
    view = fold([ev(6, started('t1'))], view);
    expect(view.approvals.a1).toMatchObject({ status: 'approved', choice: 'approve', scope: 'once' });
  });

  it('never lets a second decision replace the first', () => {
    const view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'approve', scope: 'session', reason: '' },
        { type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'changed my mind' },
        { type: 'approval/sent', approvalId: 'a1' },
        { type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'again' },
      ],
      waiting(),
    );
    expect(view.approvals.a1).toMatchObject({ status: 'sent', choice: 'approve', scope: 'session', reason: '' });
  });

  it('reopens only when delivery failed', () => {
    const failed = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'approve', scope: 'once', reason: '' },
        { type: 'approval/send-failed', approvalId: 'a1', message: 'pipe closed' },
      ],
      waiting(),
    );
    expect(failed.approvals.a1).toMatchObject({ status: 'send_failed', message: 'pipe closed' });
    const retried = fold(
      [{ type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'no' }],
      failed,
    );
    expect(retried.approvals.a1).toMatchObject({ status: 'sending', choice: 'reject', message: '' });
  });

  it("records the daemon's refusal against the approval it names", () => {
    const view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'approve', scope: 'once', reason: '' },
        { type: 'approval/sent', approvalId: 'a1' },
        ev(0, {
          kind: 'ErrorResponse',
          body: {
            code: 'ERROR_CODE_POLICY_DENIED',
            message: 'digest mismatch',
            error_details: '',
            action_id: 'a1',
          },
        }),
      ],
      waiting(),
    );
    expect(view.approvals.a1).toMatchObject({ status: 'refused', message: 'digest mismatch' });
    // An error response is outside the event sequence.
    expect(view.lastSequence).toBe(4);
  });

  it('raises an error when a rejected tool starts anyway', () => {
    const view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'no' },
        { type: 'approval/sent', approvalId: 'a1' },
        ev(5, started('t1')),
      ],
      waiting(),
    );
    expect(view.approvals.a1?.status).toBe('approved');
    const errors = Object.values(view.diagnostics).filter((d) => d.level === 'error');
    expect(errors.map((d) => d.code)).toEqual(['rejected_action_started']);
  });

  it('settles a rejection when the run moves on without the tool', () => {
    const view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'no' },
        { type: 'approval/sent', approvalId: 'a1' },
        ev(5, finished('t1', 'cancelled')),
      ],
      waiting(),
    );
    expect(view.approvals.a1?.status).toBe('rejected');
  });

  it('does not read a pause as an answer', () => {
    const view = fold(
      [
        { type: 'approval/sending', approvalId: 'a1', choice: 'reject', scope: null, reason: 'no' },
        { type: 'approval/sent', approvalId: 'a1' },
        ev(5, moved('waiting_for_approval', 'paused')),
      ],
      waiting(),
    );
    expect(view.approvals.a1?.status).toBe('sent');
  });

  it('closes open requests when the run ends', () => {
    const view = fold([ev(5, moved('waiting_for_approval', 'cancelled'))], waiting());
    expect(view.approvals.a1?.status).toBe('ended');
  });
});

describe('the rest of the stream', () => {
  it('keeps tool output up to the cap and counts the rest', () => {
    const big = new Uint8Array(MAX_TOOL_OUTPUT_BYTES - 10).fill(97);
    const view = fold([
      ev(1, proposed('t1')),
      ev(2, started('t1')),
      ev(3, { kind: 'ToolOutput', body: { tool_call_id: 't1', stream: 'stdout', chunk: big } }),
      ev(4, { kind: 'ToolOutput', body: { tool_call_id: 't1', stream: 'stdout', chunk: encode('x'.repeat(30)) } }),
      ev(5, { kind: 'ToolOutput', body: { tool_call_id: 't1', stream: 'stderr', chunk: encode('late') } }),
    ]);
    const tool = view.tools.t1;
    expect(tool?.keptBytes).toBe(MAX_TOOL_OUTPUT_BYTES);
    expect(tool?.droppedBytes).toBe(20 + 4);
    expect(tool?.segments.map((s) => [s.stream, s.bytes])).toEqual([['stdout', MAX_TOOL_OUTPUT_BYTES]]);
  });

  it('joins consecutive output from one stream and splits on a change', () => {
    const view = fold([
      ev(1, { kind: 'ToolOutput', body: { tool_call_id: 't', stream: 'stdout', chunk: encode('a') } }),
      ev(2, { kind: 'ToolOutput', body: { tool_call_id: 't', stream: 'stdout', chunk: encode('b') } }),
      ev(3, { kind: 'ToolOutput', body: { tool_call_id: 't', stream: 'stderr', chunk: encode('c') } }),
    ]);
    expect(view.tools.t?.segments.map((s) => [s.stream, s.chunks.length])).toEqual([
      ['stdout', 2],
      ['stderr', 1],
    ]);
  });

  it('accumulates messages and keeps first-seen order', () => {
    const view = fold([ev(1, said('m1', 'Hel')), ev(2, said('m2', 'Other')), ev(3, said('m1', 'lo'))]);
    expect(view.messages.m1?.text).toBe('Hello');
    expect(view.timeline).toEqual([
      { type: 'message', id: 'm1' },
      { type: 'message', id: 'm2' },
    ]);
  });

  it('keeps usage per run and never invents a cost', () => {
    const view = fold([
      ev(1, {
        kind: 'UsageUpdated',
        body: {
          run_id: 'R',
          provider: 'anthropic',
          model_sku: 'claude-sonnet-5',
          prompt_tokens: 10,
          completion_tokens: 2,
          cached_tokens: 0,
          cost_known: false,
          cost_usd: 0,
        },
      }),
    ]);
    expect(view.usage.R).toMatchObject({ cost_known: false, model_sku: 'claude-sonnet-5' });
  });

  it('warns when the daemon and this window disagree about the state a run left', () => {
    const view = fold([ev(1, moved('', 'running_model')), ev(2, moved('paused', 'running_tool'))]);
    expect(Object.values(view.diagnostics).map((d) => d.code)).toEqual(['state_mismatch']);
    expect(view.runs.R?.state).toBe('running_tool');
  });

  it('marks each new run once in the timeline', () => {
    const step = (run: string, from: string, to: string): DaemonEvent => ({
      kind: 'RunStateChanged',
      body: { run_id: run, previous_state: from, new_state: to, reason: '' },
    });
    const view = fold([
      ev(1, step('R1', '', 'queued'), { run: 'R1' }),
      ev(2, step('R1', 'queued', 'completed'), { run: 'R1' }),
      ev(3, step('R2', '', 'queued'), { run: 'R2' }),
    ]);
    expect(view.timeline.filter((item) => item.type === 'run')).toEqual([
      { type: 'run', id: 'R1' },
      { type: 'run', id: 'R2' },
    ]);
    expect(view.currentRunId).toBe('R2');
  });
});
