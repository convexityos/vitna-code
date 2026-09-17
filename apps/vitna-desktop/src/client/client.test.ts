import { describe, expect, it, vi } from 'vitest';
import type { ProtocolEnvelope } from '../protocol/types';
import { emptySession, type SessionAction, type SessionView } from '../run/model';
import { sessionReducer } from '../run/reducer';
import { SAMPLE_MARKER, SAMPLE_PROMPT } from '../sample/content';
import { SampleTransport } from '../sample/sampleTransport';
import type { LinkState, Transport, TransportHandlers } from '../transport/types';
import { VitnaClient } from './client';

/**
 * The client and reducer wired together the way useSession wires them,
 * against the scripted daemon. Everything a real bridge would carry goes
 * through the codec: envelopes, sequence numbers, error responses.
 */
function connect(transport: SampleTransport) {
  let view: SessionView = emptySession(transport.session.id);
  const links: LinkState[] = [];
  let askedAfter: number | null = null;
  const dispatch = (action: SessionAction) => {
    view = sessionReducer(view, action);
  };
  const client: VitnaClient = new VitnaClient(
    transport,
    {
      onLink: (state) => links.push(state),
      onEnvelope: (meta, decoded, receivedAt) => {
        view = sessionReducer(
          view,
          decoded.ok
            ? { type: 'event', meta, event: decoded.event, receivedAt }
            : { type: 'undecodable', meta, reason: decoded.reason, detail: decoded.detail },
        );
        if (view.gap && view.gap.after !== askedAfter) {
          askedAfter = view.gap.after;
          void client.subscribe(transport.session.id, view.gap.after);
        }
      },
    },
    { now: () => 0, newKey: () => 'key' },
  );
  return {
    client,
    links,
    dispatch,
    get view() {
      return view;
    },
  };
}

const openApproval = (view: SessionView) =>
  Object.values(view.approvals).find((approval) => approval.status === 'pending');

/** Decides the next open approval the way useSession does: mark it, send it, mark it sent. */
async function decideNext(h: ReturnType<typeof connect>, choice: 'approve' | 'reject', scope: 'once' | 'session' = 'once') {
  await vi.waitFor(() => expect(openApproval(h.view)).toBeDefined());
  const approval = openApproval(h.view)!;
  const { approval_id, action_digest } = approval.request;
  h.dispatch({ type: 'approval/sending', approvalId: approval_id, choice, scope, reason: 'not now' });
  await h.client.send(
    choice === 'approve'
      ? { kind: 'ApproveAction', body: { approval_id, action_digest, scope } }
      : { kind: 'RejectAction', body: { approval_id, action_digest, reason: 'not now' } },
    { runId: approval.runId },
  );
  h.dispatch({ type: 'approval/sent', approvalId: approval_id });
  return approval;
}

const approveNext = (h: ReturnType<typeof connect>, scope: 'once' | 'session' = 'once') =>
  decideNext(h, 'approve', scope);

async function started(transport: SampleTransport) {
  const h = connect(transport);
  await h.client.start();
  await h.client.send({
    kind: 'SubmitTurn',
    body: { session_id: transport.session.id, prompt: SAMPLE_PROMPT, attached_file_paths: [], idempotency_key: 'k' },
  });
  return h;
}

describe('VitnaClient against the scripted daemon', () => {
  it('connects, and the handshake names the session as scripted', async () => {
    const h = connect(new SampleTransport({ pace: 0 }));
    await h.client.start();
    expect(h.links).toEqual([
      { status: 'connecting' },
      { status: 'connected', daemonBuildCommit: SAMPLE_MARKER, protocol: '1.0' },
    ]);
  });

  it('plays a whole run: two approvals, four tools, a diff, a stated unknown', async () => {
    const transport = new SampleTransport({ pace: 0 });
    const h = await started(transport);
    await approveNext(h);
    await approveNext(h);
    await transport.finished;
    await vi.waitFor(() => expect(h.view.runs['run-sample-1']?.state).toBe('completed'));

    const view = h.view;
    expect(Object.values(view.tools).map((t) => [t.proposed?.tool_name, t.finished?.status])).toEqual([
      ['read_file', 'completed'],
      ['search_code', 'completed'],
      ['apply_patch', 'completed'],
      ['run_command', 'completed'],
    ]);
    expect(Object.values(view.approvals).map((a) => a.status)).toEqual(['approved', 'approved']);
    expect(Object.keys(view.diffs)).toHaveLength(1);
    expect(view.usage['run-sample-1']).toMatchObject({ cost_known: false, completion_tokens: 988 });
    expect(view.plan?.active_step_index).toBe(3);
    expect(view.messages['run-sample-1-m4']?.text).toContain('did not check');
    expect(view.gap).toBeNull();
    expect(Object.keys(view.unknown)).toEqual([]);
    // Every approval digest is a real SHA-256 in hex.
    for (const approval of Object.values(view.approvals)) {
      expect(approval.request.action_digest).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  it('refuses an approval that names the wrong digest, and leaves the request open', async () => {
    const h = await started(new SampleTransport({ pace: 0 }));
    await vi.waitFor(() => expect(openApproval(h.view)).toBeDefined());
    const approval = openApproval(h.view)!;
    await h.client.send(
      { kind: 'ApproveAction', body: { approval_id: approval.request.approval_id, action_digest: '0'.repeat(64), scope: 'once' } },
      { runId: approval.runId },
    );
    // Sent around the window's own bookkeeping, so the refusal lands as a
    // diagnostic and the request stays open for a correct answer.
    await vi.waitFor(() => expect(Object.values(h.view.diagnostics).map((d) => d.message)).toContain(
      'The digest does not match the action waiting for approval.',
    ));
    expect(h.view.tools[approval.request.tool_call_id]?.startedAtMs).toBeNull();
    // The right digest still works.
    await approveNext(h);
    await vi.waitFor(() => expect(h.view.approvals[approval.request.approval_id]?.status).toBe('approved'));
  });

  it('stops without writing when the patch is rejected', async () => {
    const transport = new SampleTransport({ pace: 0 });
    const h = await started(transport);
    const approval = await decideNext(h, 'reject');
    await transport.finished;
    await vi.waitFor(() => expect(h.view.runs['run-sample-1']?.state).toBe('completed'));
    expect(h.view.approvals[approval.request.approval_id]).toMatchObject({ status: 'rejected', reason: 'not now' });
    const patch = h.view.tools[approval.request.tool_call_id];
    expect(patch?.startedAtMs).toBeNull();
    expect(patch?.finished?.status).toBe('cancelled');
    expect(h.view.runs['run-sample-1']?.reason).toBe('the patch was not approved');
    expect(Object.values(h.view.tools).map((t) => t.proposed?.tool_name)).not.toContain('run_command');
  });

  it('recovers a withheld event through resubscription, to the same view', async () => {
    const straight = new SampleTransport({ pace: 0 });
    const a = await started(straight);
    await approveNext(a);
    await approveNext(a);
    await straight.finished;
    await vi.waitFor(() => expect(a.view.runs['run-sample-1']?.state).toBe('completed'));

    const lossy = new SampleTransport({ pace: 0 });
    lossy.withhold.add(5);
    lossy.withhold.add(12);
    const b = await started(lossy);
    await approveNext(b);
    await approveNext(b);
    await lossy.finished;
    await vi.waitFor(() => expect(b.view.runs['run-sample-1']?.state).toBe('completed'));
    await vi.waitFor(() => expect(b.view.lastSequence).toBe(a.view.lastSequence));

    expect(b.view.gap).toBeNull();
    const shape = (v: SessionView) => ({
      timeline: v.timeline,
      messages: v.messages,
      plan: v.plan,
      runs: v.runs,
      tools: Object.values(v.tools).map((t) => [t.id, t.keptBytes, t.finished?.status]),
    });
    expect(shape(b.view)).toEqual(shape(a.view));
  });

  it('reuses a session-scoped approval instead of asking again', async () => {
    const transport = new SampleTransport({ pace: 0 });
    const h = await started(transport);
    await approveNext(h, 'session');
    await approveNext(h);
    await transport.finished;
    await h.client.send({
      kind: 'SubmitTurn',
      body: { session_id: transport.session.id, prompt: 'again', attached_file_paths: [], idempotency_key: 'k2' },
    });
    await approveNext(h); // only the test command asks; the patch was approved for the session
    await transport.finished;
    await vi.waitFor(() => expect(h.view.runs['run-sample-2']?.state).toBe('completed'));
    const codes = Object.values(h.view.diagnostics).map((d) => d.code);
    expect(codes).toContain('approval_reused');
    expect(Object.values(h.view.approvals)).toHaveLength(3);
  });

  it('cancels while waiting, and closes the open request', async () => {
    const transport = new SampleTransport({ pace: 0 });
    const h = await started(transport);
    await vi.waitFor(() => expect(openApproval(h.view)).toBeDefined());
    await h.client.send({ kind: 'CancelRun', body: { run_id: 'run-sample-1', reason: 'stop' } }, { runId: 'run-sample-1' });
    await transport.finished;
    await vi.waitFor(() => expect(h.view.runs['run-sample-1']?.state).toBe('cancelled'));
    expect(Object.values(h.view.approvals).map((a) => a.status)).toEqual(['ended']);
  });
});

/** A transport that accepts everything and answers nothing, or answers with a refusal. */
class Scripted implements Transport {
  readonly kind = 'bridge' as const;
  readonly session = { id: 's', workspacePath: '/w', title: 't', mode: 'build' };
  readonly host = {};
  handlers: TransportHandlers | null = null;
  sent: ProtocolEnvelope[] = [];
  constructor(private readonly answer: ((envelope: ProtocolEnvelope) => ProtocolEnvelope | null) | null) {}
  async open(handlers: TransportHandlers) {
    this.handlers = handlers;
  }
  async send(envelope: ProtocolEnvelope) {
    this.sent.push(envelope);
    const reply = this.answer?.(envelope);
    if (reply) queueMicrotask(() => this.handlers?.onEnvelope(reply));
  }
  close() {}
}

describe('VitnaClient failure paths', () => {
  it('gives up on a daemon that never answers the handshake', async () => {
    const links: LinkState[] = [];
    const client = new VitnaClient(new Scripted(null), { onLink: (s) => links.push(s), onEnvelope: () => {} }, {
      handshakeTimeoutMs: 5,
    });
    await client.start();
    expect(links.at(-1)).toEqual({ status: 'lost', reason: 'The daemon did not answer the handshake.' });
    await expect(
      client.send({ kind: 'CancelRun', body: { run_id: 'r', reason: '' } }),
    ).rejects.toThrow('not connected');
  });

  it('reports a refused handshake with the daemon’s reason', async () => {
    const { encodeEvent } = await import('../protocol/codec');
    const refuse = (envelope: ProtocolEnvelope) =>
      encodeEvent(
        {
          kind: 'HandshakeResponse',
          body: {
            selected_version_major: 0,
            selected_version_minor: 0,
            daemon_build_commit: 'abc',
            accepted: false,
            rejection_reason: 'peer credentials do not match the daemon owner',
          },
        },
        { session_id: envelope.session_id, run_id: '', sequence: 0, idempotency_key: '' },
      );
    const links: LinkState[] = [];
    const transport = new Scripted(refuse);
    await new VitnaClient(transport, { onLink: (s) => links.push(s), onEnvelope: () => {} }).start();
    expect(links.at(-1)).toEqual({ status: 'refused', reason: 'peer credentials do not match the daemon owner' });
    // Nothing but the handshake was ever sent.
    expect(transport.sent.map((e) => e.type_url.split('.').at(-1))).toEqual(['HandshakeRequest']);
  });
});
