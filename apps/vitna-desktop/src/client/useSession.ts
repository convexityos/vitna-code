import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { ApprovalScope, SessionMode } from '../protocol/types';
import { emptySession, type SessionView } from '../run/model';
import { sessionReducer } from '../run/reducer';
import type { HostCapabilities, HostSession, LinkState, Transport } from '../transport/types';
import { VitnaClient } from './client';

export interface SessionActions {
  createSession(workspacePath: string, initialMode?: SessionMode, title?: string): Promise<HostSession | null>;
  submitTurn(prompt: string, attachedFilePaths: string[]): Promise<boolean>;
  approve(approvalId: string, scope: ApprovalScope): Promise<void>;
  reject(approvalId: string, reason: string): Promise<void>;
  steer(feedback: string): Promise<boolean>;
  pause(): Promise<boolean>;
  resume(): Promise<boolean>;
  cancel(): Promise<boolean>;
  spawnChild(goal: string): Promise<boolean>;
  applyChangeSet(changeSetId: string, expectedBaseCommit?: string): Promise<boolean>;
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
  const [activeSession, setActiveSession] = useState<HostSession | null>(transport?.session ?? null);
  const [view, dispatch] = useReducer(sessionReducer, transport?.session?.id ?? '', emptySession);
  const clientRef = useRef<VitnaClient | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const sessionRef = useRef(activeSession);
  sessionRef.current = activeSession;
  // Approvals with a decision on its way. A ref, not state, so a second key
  // press in the same frame cannot see a stale "pending" and send again.
  const inFlight = useRef(new Set<string>());

  useEffect(() => {
    setActiveSession(transport?.session ?? null);
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
    const session = activeSession;
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
  }, [gapAfter, activeSession]);

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
      const route = { sessionId: sessionRef.current?.id, runId: approval.runId };
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
      async createSession(workspacePath, initialMode = 'build', title = '') {
        const client = clientRef.current;
        if (!client || !client.isConnected) return null;
        try {
          const created = await client.createSession(workspacePath, initialMode, title);
          const next: HostSession = {
            id: created.session_id,
            workspacePath: created.workspace_path,
            title,
            mode: created.initial_mode,
          };
          setActiveSession(next);
          dispatch({ type: 'session/bound', sessionId: next.id });
          return next;
        } catch (error) {
          dispatch({
            type: 'client/diagnostic',
            level: 'error',
            code: 'create_session_failed',
            message: 'The session could not be created.',
            details: messageOf(error),
          });
          return null;
        }
      },
      async submitTurn(prompt, attachedFilePaths) {
        const client = clientRef.current;
        const session = sessionRef.current;
        if (!client || !client.isConnected || !session) return false;
        const id = globalThis.crypto.randomUUID();
        try {
          await client.send(
            {
              kind: 'SubmitTurn',
              body: { session_id: session.id, prompt, attached_file_paths: attachedFilePaths, idempotency_key: id },
            },
            { sessionId: session.id },
          );
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
            { sessionId: sessionRef.current?.id, runId },
          ),
        ),
      pause: () =>
        runCommand('Pause', (client, runId) =>
          client.send(
            { kind: 'PauseRun', body: { run_id: runId, reason: 'paused from the desktop window' } },
            { sessionId: sessionRef.current?.id, runId },
          ),
        ),
      resume: () =>
        runCommand('Resume', (client, runId) =>
          client.send({ kind: 'ResumeRun', body: { run_id: runId } }, { sessionId: sessionRef.current?.id, runId }),
        ),
      cancel: () =>
        runCommand('Cancel', (client, runId) =>
          client.send(
            { kind: 'CancelRun', body: { run_id: runId, reason: 'cancelled from the desktop window' } },
            { sessionId: sessionRef.current?.id, runId },
          ),
        ),
      spawnChild: (goal) =>
        runCommand('Spawn child', (client, runId) =>
          client.send(
            {
              kind: 'SpawnChild',
              body: {
                parent_run_id: runId,
                goal,
                requested_mode: 'build',
                token_budget: 0,
                max_nesting_depth: 0,
                idempotency_key: globalThis.crypto.randomUUID(),
              },
            },
            { sessionId: sessionRef.current?.id, runId },
          ),
        ),
      async applyChangeSet(changeSetId, expectedBaseCommit = '') {
        const client = clientRef.current;
        const session = sessionRef.current;
        if (!client || !client.isConnected || !session) return false;
        try {
          await client.send(
            {
              kind: 'ApplyChangeSet',
              body: {
                change_set_id: changeSetId,
                target_checkout_path: session.workspacePath,
                expected_base_commit: expectedBaseCommit,
                idempotency_key: globalThis.crypto.randomUUID(),
              },
            },
            { sessionId: session.id },
          );
          return true;
        } catch (error) {
          dispatch({
            type: 'client/diagnostic',
            level: 'error',
            code: 'send_failed',
            message: 'Apply changeset could not be sent.',
            details: messageOf(error),
          });
          return false;
        }
      },
    }),
    [decide, runCommand],
  );

  return {
    kind: transport?.kind ?? null,
    link,
    view,
    session: activeSession,
    host: transport?.host ?? NO_HOST,
    actions,
  };
}
