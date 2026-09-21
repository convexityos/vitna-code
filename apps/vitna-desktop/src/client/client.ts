/**
 * One connection to vitna-coded: handshake, subscription, commands.
 *
 * Two readings of the protocol are this client's own, because the .proto
 * files leave them open, and both are written down here so a daemon author
 * can hold the client to them:
 *
 * - SubscribeEvents.resume_after_sequence is proto3, so 0 and "unset" are the
 *   same value. The first subscription sends 0 and expects the daemon's
 *   sequence to start at 1, which keeps "resume after 0" meaning "from the
 *   beginning".
 * - HandshakeResponse and ErrorResponse arrive on the same stream as events.
 *   The handshake is consumed here and never reaches a session. An error
 *   response is passed on, and the session matches it to an approval through
 *   ErrorResponse.action_id, the only field the proto gives for that; any
 *   other error is shown as a diagnostic rather than pinned to a guess.
 */

import { decodeEnvelope, encodeCommand, type DecodedEnvelope } from '../protocol/codec';
import {
  PROTOCOL_VERSION_MAJOR,
  type ClientCommand,
  type HandshakeResponse,
  type ProtocolEnvelope,
} from '../protocol/types';
import type { EventMeta } from '../run/model';
import type { LinkState, Transport } from '../transport/types';

export const CLIENT_IDENTIFIER = 'vitna-desktop/0.1.0';

export interface ClientListener {
  onLink(state: LinkState): void;
  onEnvelope(meta: EventMeta, decoded: DecodedEnvelope, receivedAt: number): void;
}

export interface ClientOptions {
  handshakeTimeoutMs: number;
  now: () => number;
  newKey: () => string;
}

const DEFAULTS: ClientOptions = {
  handshakeTimeoutMs: 10_000,
  now: () => Date.now(),
  newKey: () => globalThis.crypto.randomUUID(),
};

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export class VitnaClient {
  private sequence = 0;
  private stopped = false;
  private lost = false;
  private connected = false;
  private pendingHandshake: {
    resolve: (response: HandshakeResponse) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
  } | null = null;
  private readonly options: ClientOptions;

  constructor(
    private readonly transport: Transport,
    private readonly listener: ClientListener,
    options: Partial<ClientOptions> = {},
  ) {
    this.options = { ...DEFAULTS, ...options };
  }

  get isConnected(): boolean {
    return this.connected && !this.stopped;
  }

  async start(): Promise<void> {
    this.listener.onLink({ status: 'connecting' });
    try {
      await this.transport.open({
        onEnvelope: (envelope) => this.receive(envelope),
        onClose: (reason) => this.lose(reason),
      });
    } catch (error) {
      this.lose(`The host could not open a connection to the daemon: ${messageOf(error)}`);
      return;
    }
    if (this.stopped) return;

    let response: HandshakeResponse;
    try {
      response = await this.handshake();
    } catch (error) {
      this.lose(messageOf(error));
      return;
    }
    if (this.stopped) return;

    if (!response.accepted) {
      this.refuse(response.rejection_reason || 'The daemon refused the handshake and gave no reason.');
      return;
    }
    if (response.selected_version_major !== PROTOCOL_VERSION_MAJOR) {
      this.refuse(
        `The daemon chose protocol ${response.selected_version_major}.${response.selected_version_minor}, which this window does not speak.`,
      );
      return;
    }
    this.connected = true;
    this.listener.onLink({
      status: 'connected',
      daemonBuildCommit: response.daemon_build_commit,
      protocol: `${response.selected_version_major}.${response.selected_version_minor}`,
    });

    const session = this.transport.session;
    if (session) await this.subscribe(session.id, 0);
  }

  stop(): void {
    this.stopped = true;
    this.connected = false;
    if (this.pendingHandshake) {
      clearTimeout(this.pendingHandshake.timer);
      this.pendingHandshake = null;
    }
    this.transport.close();
  }

  /** Asks for every event after `afterSequence`. Also the recovery path after a sequence gap. */
  subscribe(sessionId: string, afterSequence: number): Promise<void> {
    return this.send(
      { kind: 'SubscribeEvents', body: { session_id: sessionId, resume_after_sequence: afterSequence } },
      { sessionId },
    );
  }

  /** Sends one command. Rejects when the transport cannot deliver it; delivery says nothing about acceptance. */
  async send(command: ClientCommand, route: { sessionId?: string; runId?: string } = {}): Promise<void> {
    if (this.stopped) throw new Error('This window has closed its connection.');
    if (!this.connected && command.kind !== 'HandshakeRequest') {
      throw new Error('This window is not connected to the daemon.');
    }
    this.sequence += 1;
    const envelope = encodeCommand(command, {
      session_id: route.sessionId ?? this.transport.session?.id ?? '',
      run_id: route.runId ?? '',
      sequence: this.sequence,
      idempotency_key: this.options.newKey(),
    });
    await this.transport.send(envelope);
  }

  private handshake(): Promise<HandshakeResponse> {
    return new Promise<HandshakeResponse>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingHandshake = null;
        reject(new Error('The daemon did not answer the handshake.'));
      }, this.options.handshakeTimeoutMs);
      this.pendingHandshake = { resolve, reject, timer };
      this.send({
        kind: 'HandshakeRequest',
        body: {
          min_supported_version: PROTOCOL_VERSION_MAJOR,
          max_supported_version: PROTOCOL_VERSION_MAJOR,
          client_identifier: CLIENT_IDENTIFIER,
        },
      }).catch((error: unknown) => {
        clearTimeout(timer);
        this.pendingHandshake = null;
        reject(new Error(`The handshake could not be sent: ${messageOf(error)}`));
      });
    });
  }

  private receive(envelope: ProtocolEnvelope): void {
    if (this.stopped) return;
    const decoded = decodeEnvelope(envelope);
    if (decoded.ok && decoded.event.kind === 'HandshakeResponse') {
      const pending = this.pendingHandshake;
      if (pending) {
        clearTimeout(pending.timer);
        this.pendingHandshake = null;
        pending.resolve(decoded.event.body);
      }
      return;
    }
    if (!this.connected) return;
    this.listener.onEnvelope(
      {
        sequence: envelope.sequence,
        session_id: envelope.session_id,
        run_id: envelope.run_id,
        type_url: envelope.type_url,
      },
      decoded,
      this.options.now(),
    );
  }

  private refuse(reason: string): void {
    this.connected = false;
    this.listener.onLink({ status: 'refused', reason });
    this.transport.close();
  }

  private lose(reason: string): void {
    if (this.stopped || this.lost) return;
    this.lost = true;
    this.connected = false;
    if (this.pendingHandshake) {
      clearTimeout(this.pendingHandshake.timer);
      this.pendingHandshake.reject(new Error(reason));
      this.pendingHandshake = null;
    }
    this.listener.onLink({ status: 'lost', reason });
  }
}
