import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { ApprovalScope } from '../protocol/types';
import { emptySession, type SessionView } from '../run/model';
import { sessionReducer } from '../run/reducer';
import type { HostCapabilities, HostSession, LinkState, Transport } from '../transport/types';
import { VitnaClient } from './client';

export interface SessionActions {
  submitTurn(prompt: string, attachedFilePaths: string[]): Promise<boolean>;
  approve(approvalId: string, scope: ApprovalScope): Promise<void>;
  reject(approvalId: string, reason: string): Promise<void>;
  steer(feedback: string): Promise<boolean>;
  pause(): Promise<boolean>;
  resume(): Promise<boolean>;
  cancel(): Promise<boolean>;
}

export interface SessionHandle {
  kind: Transport['kind'] | null;
  link: LinkState;
  view: SessionView;
  session: HostSession | null;
  host: HostCapabilities;
  actions: SessionActions;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** True once an approval's deadline has passed on this window's clock. */
export function isExpired(receivedAt: number, timeoutMs: number, now: number): boolean {
  return timeoutMs > 0 && now >= receivedAt + timeoutMs;
}

const NO_HOST: HostCapabilities = {};

export function useSession(transport: Transport | null): SessionHandle {
  const [link, setLink] = useState<LinkState>(transport ? { status: 'idle' } : { status: 'no_bridge' });
  const [view, dispatch] = useReducer(sessionReducer, transport?.session?.id ?? '', emptySession);
  const clientRef = useRef<VitnaClient | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  // Approvals with a decision on its way. A ref, not state, so a second key
  // press in the same frame cannot see a stale "pending" and send again.
  const inFlight = useRef(new Set<string>());

  useEffect(() => {
    if (!transport) return undefined;
    const client = new VitnaClient(transport, {
      onLink: setLink,
      onEnvelope: (meta, decoded, receivedAt) => {
        if (decoded.ok) dispatch({ type: 'event', meta, event: decoded.event, receivedAt });
        else dispatch({ type: 'undecodable', meta, reason: decoded.reason, detail: decoded.detail });
      },
    });
    clientRef.current = client;
    void client.start();
    return () => {
      client.stop();
      clientRef.current = null;
    };
  }, [transport]);

  // A sequence gap means events were missed: ask again from the last one applied.
  const gapAfter = view.gap?.after;
  useEffect(() => {
    const client = clientRef.current;
    const session = transport?.session;
    if (gapAfter === undefined || !client || !session || !client.isConnected) return;
    client.subscribe(session.id, gapAfter).catch((error: unknown) => {
      dispatch({
        type: 'client/diagnostic',
        level: 'error',
        code: 'resubscribe_failed',
        message: 'Events were missed and this window could not ask for them again.',
        details: messageOf(error),
      });
    });
  }, [gapAfter, transport]);

  const runCommand = useCallback(
    async (label: string, send: (client: VitnaClient, runId: string) => Promise<void>): Promise<boolean> => {
      const client = clientRef.current;
      const runId = viewRef.current.currentRunId;
      if (!client || !client.isConnected || !runId) return false;
      try {
        await send(client, runId);
        return true;
      } catch (error) {
        dispatch({
          type: 'client/diagnostic',
          level: 'error',
          code: 'send_failed',
          message: `${label} could not be sent.`,
          details: messageOf(error),
        });
        return false;
      }
    },
    [],
  );

  const decide = useCallback(
    async (approvalId: string, choice: 'approve' | 'reject', scope: ApprovalScope, reason: string) => {
      const client = clientRef.current;
      const approval = viewRef.current.approvals[approvalId];
      if (!client || !client.isConnected || !approval) return;
      if (approval.status !== 'pending' && approval.status !== 'send_failed') return;
      if (isExpired(approval.receivedAt, approval.request.timeout_ms, Date.now())) return;
      if (inFlight.current.has(approvalId)) return;
      inFlight.current.add(approvalId);

      dispatch({ type: 'approval/sending', approvalId, choice, scope: choice === 'approve' ? scope : null, reason });
      const { action_digest } = approval.request;
      const route = { runId: approval.runId };
      try {
        if (choice === 'approve') {
          await client.send({ kind: 'ApproveAction', body: { approval_id: approvalId, action_digest, scope } }, route);
        } else {
          await client.send({ kind: 'RejectAction', body: { approval_id: approvalId, action_digest, reason } }, route);
        }
        dispatch({ type: 'approval/sent', approvalId });
      } catch (error) {
        dispatch({ type: 'approval/send-failed', approvalId, message: messageOf(error) });
      } finally {
        inFlight.current.delete(approvalId);
      }
    },
    [],
  );

  const actions = useMemo<SessionActions>(
    () => ({
      async submitTurn(prompt, attachedFilePaths) {
        const client = clientRef.current;
        const session = transport?.session;
        if (!client || !client.isConnected || !session) return false;
        const id = globalThis.crypto.randomUUID();
        try {
          await client.send({
            kind: 'SubmitTurn',
            body: { session_id: session.id, prompt, attached_file_paths: attachedFilePaths, idempotency_key: id },
          });
          dispatch({ type: 'turn/sent', id, prompt, sentAt: Date.now() });
          return true;
        } catch (error) {
          dispatch({
            type: 'client/diagnostic',
            level: 'error',
            code: 'send_failed',
            message: 'The prompt could not be sent.',
            details: messageOf(error),
          });
          return false;
        }
      },
      approve: (approvalId, scope) => decide(approvalId, 'approve', scope, ''),
      reject: (approvalId, reason) => decide(approvalId, 'reject', 'once', reason),
      steer: (feedback) =>
        runCommand('The steer', (client, runId) =>
          client.send(
            { kind: 'SteerRun', body: { run_id: runId, feedback, idempotency_key: globalThis.crypto.randomUUID() } },
            { runId },
          ),
        ),
      pause: () =>
        runCommand('Pause', (client, runId) =>
          client.send({ kind: 'PauseRun', body: { run_id: runId, reason: 'paused from the desktop window' } }, { runId }),
        ),
      resume: () =>
        runCommand('Resume', (client, runId) => client.send({ kind: 'ResumeRun', body: { run_id: runId } }, { runId })),
      cancel: () =>
        runCommand('Cancel', (client, runId) =>
          client.send({ kind: 'CancelRun', body: { run_id: runId, reason: 'cancelled from the desktop window' } }, { runId }),
        ),
    }),
    [decide, runCommand, transport],
  );

  return {
    kind: transport?.kind ?? null,
    link,
    view,
    session: transport?.session ?? null,
    host: transport?.host ?? NO_HOST,
    actions,
  };
}
