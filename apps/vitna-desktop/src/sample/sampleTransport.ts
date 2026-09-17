/**
 * A scripted daemon, for development only.
 *
 * It speaks the same envelopes a bridge would carry, so the sample exercises
 * the real client, codec and reducer rather than a parallel path: handshake,
 * sequence numbers, resubscription after a gap, approval gates that refuse a
 * wrong digest, pause, resume and cancel. What it does not do is run
 * anything. Every event is a line of the script in src/sample/content.ts, and
 * the handshake names itself as scripted so the window can say so.
 */

import { canonicalJson } from '../receipt/receipt';
import { bytesToHex } from '../receipt/verify';
import { decodeCommandPayload, encodeEvent } from '../protocol/codec';
import {
  APPROVAL_SCOPES,
  type ApprovalRequested,
  type ApprovalScope,
  type DaemonEvent,
  type ProtocolEnvelope,
  type ToolProposed,
} from '../protocol/types';
import type {
  HostCapabilities,
  HostProvider,
  HostSession,
  HostWorkspace,
  Transport,
  TransportHandlers,
} from '../transport/types';
import {
  MESSAGES,
  PLAN,
  PYTEST_ARGV,
  PYTEST_OUTPUT,
  SAMPLE_MARKER,
  SAMPLE_WORKSPACE,
  SEARCH_OUTPUT,
  SERVICE_BEFORE,
  SERVICE_DIFF,
  SERVICE_PATH,
} from './content';

const encoder = new TextEncoder();

export async function sha256Hex(data: string | Uint8Array): Promise<string> {
  const bytes = typeof data === 'string' ? encoder.encode(data) : data;
  const digest = await globalThis.crypto.subtle.digest('SHA-256', bytes);
  return bytesToHex(new Uint8Array(digest));
}

export const SAMPLE_PROVIDERS: HostProvider[] = [
  {
    id: 'anthropic',
    name: 'Anthropic',
    description: 'Claude models, called directly',
    credentialSource: 'environment variable ANTHROPIC_API_KEY',
  },
  {
    id: 'openai',
    name: 'OpenAI',
    description: 'GPT models, called directly',
    credentialSource: null,
  },
];

const SAMPLE_WORKSPACES: HostWorkspace[] = [
  { path: SAMPLE_WORKSPACE, name: 'ratelimit' },
  { path: '/home/dev/billing', name: 'billing' },
];

type Decision =
  | { kind: 'approve'; scope: ApprovalScope }
  | { kind: 'reject'; reason: string }
  | { kind: 'expired' }
  | { kind: 'cancel' };

interface Gate {
  approvalId: string;
  digest: string;
  resolve: (decision: Decision) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

class Cancelled extends Error {}

export interface SampleOptions {
  /** Multiplies every scripted delay. 0 runs the script as fast as promises settle (tests). */
  pace?: number;
  /** Approval deadline in milliseconds; 0 sends no deadline. */
  approvalTimeoutMs?: number;
}

export class SampleTransport implements Transport {
  readonly kind = 'sample' as const;
  readonly session: HostSession = {
    id: 'sess-sample',
    workspacePath: SAMPLE_WORKSPACE,
    title: 'Rate limiter fails open',
    mode: 'build',
  };
  readonly host: HostCapabilities = {
    listWorkspaces: async () => SAMPLE_WORKSPACES,
    listProviders: async () => SAMPLE_PROVIDERS,
  };

  /** Test hook: sequences listed here are withheld once from live delivery, to exercise gap recovery. */
  readonly withhold = new Set<number>();

  private handlers: TransportHandlers | null = null;
  private readonly log: ProtocolEnvelope[] = [];
  private sequence = 0;
  private subscribed = false;
  private closed = false;
  private runs = 0;
  private runId = '';
  private state = '';
  private stateBeforePause = '';
  private active = false;
  private cancelled = false;
  private paused = false;
  private wakers: (() => void)[] = [];
  private gate: Gate | null = null;
  private approvals = 0;
  private readonly remembered = new Map<string, ApprovalScope>();
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private readonly pace: number;
  private readonly approvalTimeoutMs: number;
  /** Settles when the current script finishes. Tests await it. */
  finished: Promise<void> = Promise.resolve();

  constructor(options: SampleOptions = {}) {
    this.pace = options.pace ?? 1;
    this.approvalTimeoutMs = options.approvalTimeoutMs ?? 5 * 60 * 1000;
  }

  async open(handlers: TransportHandlers): Promise<void> {
    // Reopening is normal: React's StrictMode closes and reopens every
    // connection once in development. The log survives, so a resubscribe
    // after reopening replays the run so far.
    this.closed = false;
    this.subscribed = false;
    this.handlers = handlers;
  }

  close(): void {
    this.closed = true;
    this.subscribed = false;
    this.handlers = null;
  }

  async send(envelope: ProtocolEnvelope): Promise<void> {
    if (this.closed) throw new Error('The sample session is closed.');
    const { kind, body } = decodeCommandPayload(envelope);
    switch (kind) {
      case 'HandshakeRequest':
        this.reply({
          kind: 'HandshakeResponse',
          body: {
            selected_version_major: 1,
            selected_version_minor: 0,
            daemon_build_commit: SAMPLE_MARKER,
            accepted: true,
            rejection_reason: '',
          },
        });
        return;
      case 'SubscribeEvents': {
        const after = typeof body.resume_after_sequence === 'number' ? body.resume_after_sequence : 0;
        this.subscribed = true;
        for (const past of this.log) if (past.sequence > after) this.deliver(past);
        return;
      }
      case 'SubmitTurn': {
        if (this.active) {
          this.emit({
            kind: 'Diagnostic',
            body: {
              level: 'warn',
              code: 'run_active',
              message: 'A run is already active in this session. Wait for it, or cancel it.',
              details: '',
            },
          });
          return;
        }
        const prompt = typeof body.prompt === 'string' ? body.prompt : '';
        this.finished = this.play(prompt);
        return;
      }
      case 'ApproveAction':
      case 'RejectAction':
        this.answer(kind, body);
        return;
      case 'PauseRun':
        if (this.active && !this.paused) {
          this.paused = true;
          this.stateBeforePause = this.state;
          this.moveTo('paused', 'paused by the operator');
        }
        return;
      case 'ResumeRun':
        if (this.active && this.paused) {
          this.paused = false;
          this.moveTo(this.stateBeforePause, 'resumed by the operator');
          this.wake();
        }
        return;
      case 'CancelRun':
        if (this.active) {
          this.cancelled = true;
          if (this.gate) {
            if (this.gate.timer) clearTimeout(this.gate.timer);
            this.gate.resolve({ kind: 'cancel' });
            this.gate = null;
          }
          this.paused = false;
          this.wake();
        }
        return;
      case 'SteerRun':
        this.emit({
          kind: 'Diagnostic',
          body: {
            level: 'info',
            code: 'sample_script',
            message: 'The sample plays a fixed script, so steering is received and not acted on.',
            details: typeof body.feedback === 'string' ? body.feedback : '',
          },
        });
        return;
      default:
        this.reply({
          kind: 'ErrorResponse',
          body: {
            code: 'ERROR_CODE_UNKNOWN_COMMAND',
            message: `The sample session does not handle ${kind}.`,
            error_details: '',
            action_id: '',
          },
        });
    }
  }

  // ---- wire ------------------------------------------------------------------

  private deliver(envelope: ProtocolEnvelope): void {
    const handlers = this.handlers;
    if (!handlers || this.closed) return;
    // A pipe is asynchronous; keep delivery off the sender's stack, in order.
    void Promise.resolve().then(() => {
      if (!this.closed) handlers.onEnvelope(envelope);
    });
  }

  /** A response to a command. Unsequenced: it answers the command rather than extending a run. */
  private reply(event: DaemonEvent): void {
    this.deliver(
      encodeEvent(event, { session_id: this.session.id, run_id: this.runId, sequence: 0, idempotency_key: '' }),
    );
  }

  /** Appends to the log whether or not anyone is connected, the way a daemon persists before it delivers. */
  private emit(event: DaemonEvent): void {
    this.sequence += 1;
    const envelope = encodeEvent(event, {
      session_id: this.session.id,
      run_id: this.runId,
      sequence: this.sequence,
      idempotency_key: '',
    });
    this.log.push(envelope);
    if (!this.subscribed || this.closed) return;
    if (this.withhold.delete(envelope.sequence)) return;
    this.deliver(envelope);
  }

  private moveTo(next: string, reason: string): void {
    const previous = this.state;
    this.state = next;
    this.emit({
      kind: 'RunStateChanged',
      body: { run_id: this.runId, previous_state: previous, new_state: next, reason },
    });
  }

  // ---- pacing ----------------------------------------------------------------

  private wake(): void {
    const wakers = this.wakers;
    this.wakers = [];
    for (const wake of wakers) wake();
  }

  private async wait(ms: number): Promise<void> {
    if (this.pace > 0 && ms > 0) {
      await new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          this.timers.delete(timer);
          resolve();
        }, ms * this.pace);
        this.timers.add(timer);
      });
    } else {
      await Promise.resolve();
    }
    while (this.paused && !this.cancelled) {
      await new Promise<void>((resolve) => this.wakers.push(resolve));
    }
    if (this.cancelled) throw new Cancelled();
  }

  private async say(messageId: string, text: string): Promise<void> {
    const words = text.split(/(?<= )/);
    const size = Math.max(1, Math.ceil(words.length / 6));
    for (let i = 0; i < words.length; i += size) {
      this.emit({
        kind: 'MessageDelta',
        body: { message_id: messageId, role: 'model', delta_text: words.slice(i, i + size).join('') },
      });
      await this.wait(70);
    }
  }

  // ---- tools and approvals ----------------------------------------------------

  private async propose(name: string, args: unknown, effect: string): Promise<ToolProposed> {
    const raw = encoder.encode(JSON.stringify(args));
    const call: ToolProposed = {
      tool_call_id: `${this.runId}-call-${this.sequence + 1}`,
      tool_name: name,
      tool_version: '1.0.0',
      definition_digest: await sha256Hex(`vitna-tool ${name}@1.0.0`),
      argument_digest: await sha256Hex(raw),
      effect_class: effect,
      raw_arguments_json: raw,
    };
    this.emit({ kind: 'ToolProposed', body: call });
    return call;
  }

  private async execute(call: ToolProposed, output: string[], durationMs: number): Promise<void> {
    this.emit({ kind: 'ToolStarted', body: { tool_call_id: call.tool_call_id, started_at_ms: 0 } });
    for (const chunk of output) {
      await this.wait(durationMs / (output.length + 1));
      this.emit({
        kind: 'ToolOutput',
        body: { tool_call_id: call.tool_call_id, stream: 'stdout', chunk: encoder.encode(chunk) },
      });
    }
    await this.wait(durationMs / (output.length + 1));
    this.emit({
      kind: 'ToolFinished',
      body: {
        tool_call_id: call.tool_call_id,
        exit_code: 0,
        stdout_digest: await sha256Hex(output.join('')),
        stderr_digest: await sha256Hex(''),
        duration_ms: durationMs,
        status: 'completed',
      },
    });
  }

  private async approval(
    call: ToolProposed,
    fields: Omit<ApprovalRequested, 'approval_id' | 'tool_call_id' | 'action_digest' | 'timeout_ms'>,
  ): Promise<Decision> {
    const scope = this.remembered.get(call.tool_name);
    if (scope) {
      this.emit({
        kind: 'Diagnostic',
        body: {
          level: 'info',
          code: 'approval_reused',
          message: `${call.tool_name} was approved for this ${scope} earlier, so no new approval is asked for.`,
          details: '',
        },
      });
      this.moveTo('running_tool', `approved earlier for this ${scope}`);
      return { kind: 'approve', scope };
    }

    const previous = this.state;
    this.moveTo('waiting_for_approval', 'this action needs your approval');
    this.approvals += 1;
    const approvalId = `${this.runId}-approval-${this.approvals}`;
    const timeout = this.approvalTimeoutMs;
    // The sample's own recipe, over the inputs CONTRIBUTING.md rule 3 lists.
    // The daemon has not fixed one, so nothing here claims to be its digest.
    const digest = await sha256Hex(
      canonicalJson({
        tool: `${call.tool_name}@${call.tool_version}`,
        definition_digest: call.definition_digest,
        argument_digest: call.argument_digest,
        executable_identity: fields.executable_identity,
        canonical_cwd: fields.canonical_cwd,
        environment_names: fields.environment_names,
        bound_mounts: fields.bound_mounts,
        timeout_ms: timeout,
      }),
    );
    this.emit({
      kind: 'ApprovalRequested',
      body: {
        ...fields,
        approval_id: approvalId,
        tool_call_id: call.tool_call_id,
        action_digest: digest,
        timeout_ms: timeout,
      },
    });

    const decision = await new Promise<Decision>((resolve) => {
      const timer =
        timeout > 0
          ? setTimeout(() => {
              if (this.gate?.approvalId === approvalId) {
                this.gate = null;
                resolve({ kind: 'expired' });
              }
            }, timeout)
          : null;
      this.gate = { approvalId, digest, resolve, timer };
    });

    if (decision.kind === 'approve') {
      if (decision.scope !== 'once') this.remembered.set(call.tool_name, decision.scope);
      this.moveTo('running_tool', `approved ${decision.scope === 'once' ? 'once' : `for this ${decision.scope}`}`);
    } else if (decision.kind === 'reject') {
      this.emit({
        kind: 'ToolFinished',
        body: {
          tool_call_id: call.tool_call_id,
          exit_code: 0,
          stdout_digest: '',
          stderr_digest: '',
          duration_ms: 0,
          status: 'cancelled',
        },
      });
      this.moveTo(previous, decision.reason ? `rejected: ${decision.reason}` : 'rejected by the operator');
    } else if (decision.kind === 'expired') {
      this.emit({
        kind: 'Diagnostic',
        body: {
          level: 'warn',
          code: 'approval_expired',
          message: 'The approval request expired before anyone answered it.',
          details: approvalId,
        },
      });
    }
    return decision;
  }

  private answer(kind: 'ApproveAction' | 'RejectAction', body: Record<string, unknown>): void {
    const approvalId = typeof body.approval_id === 'string' ? body.approval_id : '';
    const refuse = (message: string) =>
      this.reply({
        kind: 'ErrorResponse',
        body: { code: 'ERROR_CODE_POLICY_DENIED', message, error_details: '', action_id: approvalId },
      });
    const gate = this.gate;
    if (!gate || gate.approvalId !== approvalId) {
      refuse('No approval with that id is waiting.');
      return;
    }
    if (body.action_digest !== gate.digest) {
      refuse('The digest does not match the action waiting for approval.');
      return;
    }
    if (kind === 'ApproveAction') {
      const scope = body.scope;
      if (typeof scope !== 'string' || !(APPROVAL_SCOPES as readonly string[]).includes(scope)) {
        refuse('An approval scope must be once, session or project.');
        return;
      }
      this.settle(gate, { kind: 'approve', scope: scope as ApprovalScope });
    } else {
      this.settle(gate, { kind: 'reject', reason: typeof body.reason === 'string' ? body.reason : '' });
    }
  }

  private settle(gate: Gate, decision: Decision): void {
    if (gate.timer) clearTimeout(gate.timer);
    this.gate = null;
    gate.resolve(decision);
  }

  // ---- the script --------------------------------------------------------------

  private async play(prompt: string): Promise<void> {
    this.runs += 1;
    this.runId = `run-sample-${this.runs}`;
    this.active = true;
    this.cancelled = false;
    this.paused = false;
    this.state = '';
    const run = this.runId;
    try {
      this.moveTo('queued', 'turn submitted');
      this.emit({ kind: 'MessageDelta', body: { message_id: `${run}-user`, role: 'user', delta_text: prompt } });
      await this.wait(350);
      this.moveTo('running_model', 'context assembled');
      this.emit({ kind: 'PlanChanged', body: { plan_id: `${run}-plan`, steps: PLAN, active_step_index: 0 } });
      this.usage(6120, 0, 0);
      await this.wait(500);
      await this.say(`${run}-m1`, MESSAGES.opening);
      this.emit({
        kind: 'ReasoningSummaryDelta',
        body: {
          step_id: `${run}-r1`,
          delta_text: 'The Redis read has no deadline, and the tests already describe a stalled-Redis case.',
        },
      });

      const read = await this.propose('read_file', { path: SERVICE_PATH }, 'read');
      await this.wait(250);
      this.moveTo('running_tool', 'read-only tool');
      await this.execute(read, [SERVICE_BEFORE], 3);

      const search = await this.propose('search_code', { query: 'check_rate_limit', path: 'tests' }, 'read');
      await this.wait(250);
      await this.execute(search, [SEARCH_OUTPUT], 41);
      this.moveTo('running_model', 'tool results returned');
      this.usage(9870, 412, 5824);

      await this.wait(600);
      this.emit({ kind: 'PlanChanged', body: { plan_id: `${run}-plan`, steps: PLAN, active_step_index: 1 } });
      await this.say(`${run}-m2`, MESSAGES.proposal);
      const patch = await this.propose('apply_patch', { path: SERVICE_PATH, patch: SERVICE_DIFF }, 'write');
      this.emit({
        kind: 'DiffChanged',
        body: {
          agent_workspace_id: `${run}-workspace`,
          diff_text: SERVICE_DIFF,
          diff_digest: await sha256Hex(SERVICE_DIFF),
          modified_files: [SERVICE_PATH],
        },
      });
      await this.wait(300);
      const patched = await this.approval(patch, {
        description: `Write 13 added lines and 1 removed line to ${SERVICE_PATH}`,
        executable_identity: 'vitna-runner apply_patch 1.0.0',
        canonical_cwd: SAMPLE_WORKSPACE,
        environment_names: [],
        bound_mounts: [`${SAMPLE_WORKSPACE}/src (read-write)`],
      });
      if (patched.kind !== 'approve') return await this.stop(patched, MESSAGES.rejected, 'the patch was not approved');
      await this.execute(patch, [], 12);
      this.moveTo('running_model', 'patch written');

      await this.wait(400);
      this.emit({ kind: 'PlanChanged', body: { plan_id: `${run}-plan`, steps: PLAN, active_step_index: 2 } });
      await this.say(`${run}-m3`, MESSAGES.afterPatch);
      const test = await this.propose('run_command', { argv: PYTEST_ARGV, cwd: '.' }, 'execute');
      await this.wait(300);
      const tested = await this.approval(test, {
        description: `Run ${PYTEST_ARGV.join(' ')}`,
        executable_identity: '/usr/bin/python3.12',
        canonical_cwd: SAMPLE_WORKSPACE,
        environment_names: ['PATH', 'PYTHONPATH', 'REDIS_URL'],
        bound_mounts: [`${SAMPLE_WORKSPACE} (read-only)`, `/tmp/vitna/${run} (read-write)`],
      });
      if (tested.kind !== 'approve') {
        return await this.stop(tested, 'Understood. The patch is written and untested.', 'the tests were not run');
      }
      await this.execute(test, PYTEST_OUTPUT, 1830);
      this.moveTo('running_model', 'tests finished');

      await this.wait(400);
      this.emit({ kind: 'PlanChanged', body: { plan_id: `${run}-plan`, steps: PLAN, active_step_index: 3 } });
      this.usage(14210, 988, 9380);
      await this.say(`${run}-m4`, MESSAGES.done);
      this.moveTo('completed', 'finished with one unknown stated');
    } catch (error) {
      if (!(error instanceof Cancelled) || this.closed) return;
      this.cancelled = false;
      this.moveTo('cancelled', 'cancelled by the operator');
    } finally {
      this.active = false;
    }
  }

  private async stop(decision: Decision, message: string, reason: string): Promise<void> {
    if (decision.kind === 'cancel') throw new Cancelled();
    if (decision.kind === 'expired') {
      this.moveTo('cancelled', 'the approval request expired');
      return;
    }
    await this.say(`${this.runId}-stop`, message);
    this.moveTo('completed', reason);
  }

  private usage(prompt: number, completion: number, cached: number): void {
    this.emit({
      kind: 'UsageUpdated',
      body: {
        run_id: this.runId,
        provider: 'anthropic',
        model_sku: 'claude-sonnet-5',
        prompt_tokens: prompt,
        completion_tokens: completion,
        cached_tokens: cached,
        cost_known: false,
        cost_usd: 0,
      },
    });
  }
}
