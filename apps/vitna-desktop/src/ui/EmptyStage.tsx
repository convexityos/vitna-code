import { useEffect, useState, type ReactNode } from 'react';
import type { HostCapabilities, HostProvider, HostSession, LinkState } from '../transport/types';
import { ChevronRightIcon } from './icons';
import { WorkspaceMenu } from './WorkspaceMenu';

interface EmptyStageProps {
  link: LinkState;
  session: HostSession | null;
  host: HostCapabilities;
  composer: ReactNode;
  onOpenSample: (() => void) | null;
  sampleError: string;
  onProviders(): void;
}

/**
 * A session with nothing in it yet: the mark, the composer, the workspace,
 * and the providers. When there is no daemon, the same page says so plainly
 * and keeps the composer visible but closed.
 */
export function EmptyStage({ link, session, host, composer, onOpenSample, sampleError, onProviders }: EmptyStageProps) {
  return (
    <section className="empty-stage" aria-labelledby="empty-title">
      <h1 id="empty-title" className="visually-hidden">
        Vitna Code
      </h1>
      <p className="wordmark" aria-hidden="true">
        vitna<span className="wordmark-gap"> </span>code
      </p>

      <div className="empty-body">
        {composer}
        <div className="empty-under">
          <WorkspaceMenu session={session} host={host} />
          <p className="empty-line mono fainter">The coding terminal that leaves a receipt.</p>
        </div>

        {link.status === 'no_bridge' ? (
          <div className="no-daemon" role="note">
            <p className="no-daemon-title">No daemon connected</p>
            <p>
              This window reaches <code>vitna-coded</code> only through a bridge the Vitna Code desktop shell provides, and
              none is present here. A browser page cannot open the daemon&rsquo;s pipe or socket by design (ADR-0005).
            </p>
            {onOpenSample ? (
              <p className="no-daemon-sample">
                <button type="button" className="pill pill-small" onClick={onOpenSample}>
                  Open the sample session
                </button>
                <span className="faint">
                  Development only. A scripted run through the real protocol path; nothing executes.
                </span>
              </p>
            ) : null}
            {sampleError ? <p className="tone-rust">The sample did not load: {sampleError}</p> : null}
          </div>
        ) : null}
        {link.status === 'refused' || link.status === 'lost' ? (
          <div className="no-daemon" role="alert">
            <p className="no-daemon-title tone-rust">
              {link.status === 'refused' ? 'The daemon refused this window' : 'The connection was lost'}
            </p>
            <p>{link.reason}</p>
          </div>
        ) : null}
      </div>

      <ProvidersLine host={host} onOpen={onProviders} />
    </section>
  );
}

function ProvidersLine({ host, onOpen }: { host: HostCapabilities; onOpen(): void }) {
  const [providers, setProviders] = useState<HostProvider[] | null>(null);
  const list = host.listProviders;

  useEffect(() => {
    let live = true;
    if (!list) return undefined;
    list()
      .then((items) => {
        if (live) setProviders(items);
      })
      .catch(() => {
        if (live) setProviders([]);
      });
    return () => {
      live = false;
    };
  }, [list]);

  let sentence: string;
  if (!list) {
    sentence = 'Model providers are set up on the host';
  } else if (providers === null) {
    sentence = 'Reading the host’s model providers';
  } else {
    const ready = providers.filter((p) => p.credentialSource !== null);
    sentence =
      providers.length === 0
        ? 'The host lists no model providers'
        : `${ready.length} of ${providers.length} model providers ready: ${
            ready.map((p) => p.name).join(', ') || 'none yet'
          }`;
  }

  return (
    <footer className="empty-foot">
      <button type="button" className="empty-foot-link" onClick={onOpen}>
        {sentence}
        <ChevronRightIcon />
      </button>
    </footer>
  );
}
