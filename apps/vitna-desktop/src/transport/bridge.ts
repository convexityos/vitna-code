/**
 * The one way this page reaches vitna-coded: a bridge object a desktop shell
 * puts on `window` before the page loads.
 *
 * ADR-0005 keeps the daemon on an owner-only Unix socket or named pipe and
 * never on loopback TCP, precisely so that no web page can reach it. A page
 * cannot open either, so the shell holds the pipe and hands this page raw
 * bytes. The interface below is this app's side of that contract; no shell
 * implements it yet, and in a plain browser `window.vitnaBridge` is simply
 * absent, which the app reports as "no bridge" rather than guessing.
 *
 * Bytes, not objects, cross the bridge, so the shell stays a dumb pipe and
 * the framing rules in src/protocol/frame.ts are enforced here, on the side
 * that renders what arrives.
 */

import { encodeFrame, FrameDecoder } from '../protocol/frame';
import type { ProtocolEnvelope } from '../protocol/types';
import type { HostCapabilities, HostSession, Transport, TransportHandlers } from './types';

export interface VitnaBridge extends HostCapabilities {
  readonly apiVersion: 1;
  /** The session the shell opened this window for, if any. */
  readonly session: HostSession | null;
  /** Opens the pipe. `onBytes` receives the daemon's byte stream in any chunking. */
  connect(onBytes: (chunk: Uint8Array) => void, onClose: (reason: string) => void): Promise<void>;
  /** Writes one complete frame. */
  write(frame: Uint8Array): Promise<void>;
  disconnect(): void;
}

declare global {
  interface Window {
    vitnaBridge?: VitnaBridge;
  }
}

export function findBridge(): VitnaBridge | null {
  if (typeof window === 'undefined') return null;
  const bridge = window.vitnaBridge;
  if (!bridge || bridge.apiVersion !== 1) return null;
  return bridge;
}

export class BridgeTransport implements Transport {
  readonly kind = 'bridge' as const;
  private closed = false;
  private generation = 0;

  constructor(private readonly bridge: VitnaBridge) {}

  get session(): HostSession | null {
    return this.bridge.session;
  }

  get host(): HostCapabilities {
    const b = this.bridge;
    // Bound one by one so a host method keeps its `this`, and a missing one stays missing.
    return {
      newSession: b.newSession?.bind(b),
      listWorkspaces: b.listWorkspaces?.bind(b),
      openWorkspace: b.openWorkspace?.bind(b),
      pickFiles: b.pickFiles?.bind(b),
      listProviders: b.listProviders?.bind(b),
      connectProvider: b.connectProvider?.bind(b),
    };
  }

  async open(handlers: TransportHandlers): Promise<void> {
    // A reopened connection starts clean: no half frame from the last one,
    // and nothing the last one still delivers is read.
    this.generation += 1;
    const generation = this.generation;
    const current = () => generation === this.generation && !this.closed;
    this.closed = false;
    const decoder = new FrameDecoder();
    await this.bridge.connect(
      (chunk) => {
        if (!current()) return;
        const batch = decoder.push(chunk);
        for (const envelope of batch.envelopes) handlers.onEnvelope(envelope);
        if (batch.error) {
          // ADR-0005 step 1: a bad frame ends the connection.
          this.close();
          handlers.onClose(`The daemon sent a frame this window could not read: ${batch.error.message}`);
        }
      },
      (reason) => {
        if (!current()) return;
        this.closed = true;
        handlers.onClose(reason);
      },
    );
  }

  async send(envelope: ProtocolEnvelope): Promise<void> {
    if (this.closed) throw new Error('The connection to the daemon is closed.');
    await this.bridge.write(encodeFrame(envelope));
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.bridge.disconnect();
  }
}
