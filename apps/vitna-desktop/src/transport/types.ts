import type { ProtocolEnvelope } from '../protocol/types';

/**
 * The session a window was opened for. The host creates it (the way `vitna`
 * does when it starts in a directory), because commands.proto declares
 * CreateSession but no message that answers it with the new session's id.
 */
export interface HostSession {
  id: string;
  workspacePath: string;
  title: string;
  /** CreateSession.initial_mode, as the host created it. */
  mode: string;
  /** The checked-out branch, when the host knows it. The protocol carries none, so this is the host's fact alone. */
  branch?: string;
}

export interface HostWorkspace {
  path: string;
  name: string;
}

/**
 * A model provider as the host sees it. The page learns only whether the host
 * can resolve credentials for it, never the credentials: the Honesty Contract
 * keeps provider keys with the customer, and a web page is the last place a
 * key should be typed.
 */
export interface HostProvider {
  id: string;
  name: string;
  description: string;
  /** How the host resolves this provider's credentials, or null when it cannot. */
  credentialSource: string | null;
}

/**
 * Things only the host can do. Each is optional: a host that lacks one leaves
 * it out, and the page says the host does not offer it rather than pretending.
 */
export interface HostCapabilities {
  newSession?(): Promise<void>;
  listWorkspaces?(): Promise<HostWorkspace[]>;
  /** With no path, the host shows its own folder picker. */
  openWorkspace?(path?: string): Promise<void>;
  /** Returns paths relative to the session's workspace. */
  pickFiles?(): Promise<string[]>;
  listProviders?(): Promise<HostProvider[]>;
  /** The host runs its own credential flow (keychain, workload identity, helper). */
  connectProvider?(id: string): Promise<void>;
}

export type LinkState =
  | { status: 'idle' }
  | { status: 'no_bridge' }
  | { status: 'connecting' }
  | { status: 'connected'; daemonBuildCommit: string; protocol: string }
  | { status: 'refused'; reason: string }
  | { status: 'lost'; reason: string };

export interface TransportHandlers {
  onEnvelope(envelope: ProtocolEnvelope): void;
  onClose(reason: string): void;
}

/**
 * Envelopes in, envelopes out. Framing, pipes and sockets live below this line.
 */
export interface Transport {
  /** "bridge" reaches vitna-coded through a desktop shell. "sample" is the DEV-only scripted run. */
  readonly kind: 'bridge' | 'sample';
  /** Null when the host did not open this window for a session. */
  readonly session: HostSession | null;
  readonly host: HostCapabilities;
  open(handlers: TransportHandlers): Promise<void>;
  send(envelope: ProtocolEnvelope): Promise<void>;
  close(): void;
}
