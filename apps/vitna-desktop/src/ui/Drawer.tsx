import { useEffect, useRef } from 'react';
import type { ViewName } from '../app/Workbench';
import { CLIENT_IDENTIFIER } from '../client/client';
import { PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR } from '../protocol/types';
import type { HostSession, LinkState } from '../transport/types';
import { CloseIcon } from './icons';
import { basename } from './text';

interface DrawerProps {
  session: HostSession | null;
  link: LinkState;
  isSample: boolean;
  activeView: ViewName;
  onView(view: ViewName): void;
  onProviders(): void;
  onClose(): void;
}

function linkSentence(link: LinkState, isSample: boolean): string {
  switch (link.status) {
    case 'connected':
      return isSample ? 'A scripted sample, not a daemon.' : `Connected, protocol ${link.protocol}.`;
    case 'no_bridge':
      return 'No bridge to vitna-coded in this window.';
    case 'idle':
    case 'connecting':
      return 'Connecting.';
    case 'refused':
      return `Refused: ${link.reason}`;
    case 'lost':
      return `Lost: ${link.reason}`;
  }
}

export function Drawer({ session, link, isSample, activeView, onView, onProviders, onClose }: DrawerProps) {
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('button')?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <>
      <div className="drawer-scrim" onClick={onClose} aria-hidden="true" />
      <aside className="drawer" ref={ref} aria-label="Session and views">
        <div className="drawer-identity">
          <div>
            <p className="drawer-brand">Vitna Code</p>
            <p className="drawer-link mono">vitna-coded · {linkSentence(link, isSample)}</p>
          </div>
          <button type="button" className="pill icon-pill pill-small" aria-label="Close" onClick={onClose}>
            <CloseIcon />
          </button>
        </div>

        <nav className="drawer-groups">
          <section className="drawer-group">
            <h2 className="eyebrow">Session</h2>
            {session ? (
              <div className="drawer-session">
                <p className="drawer-session-title">{session.title}</p>
                <p className="mono faint" title={session.workspacePath}>
                  {basename(session.workspacePath)} · {session.mode || 'mode not stated'}
                </p>
              </div>
            ) : (
              <p className="drawer-empty">No session is open in this window.</p>
            )}
          </section>

          <section className="drawer-group">
            <h2 className="eyebrow">Views</h2>
            <ul>
              <li>
                <button
                  type="button"
                  className="drawer-row"
                  aria-current={activeView === 'session' ? 'page' : undefined}
                  onClick={() => onView('session')}
                >
                  Session
                </button>
              </li>
              <li>
                <button
                  type="button"
                  className="drawer-row"
                  aria-current={activeView === 'receipt' ? 'page' : undefined}
                  onClick={() => onView('receipt')}
                >
                  Receipt reader
                </button>
              </li>
            </ul>
          </section>

          <section className="drawer-group">
            <h2 className="eyebrow">Host</h2>
            <ul>
              <li>
                <button type="button" className="drawer-row" onClick={onProviders}>
                  Model providers
                </button>
              </li>
            </ul>
          </section>
        </nav>

        <p className="drawer-foot mono fainter">
          {CLIENT_IDENTIFIER} · Vitna Agent Protocol {PROTOCOL_VERSION_MAJOR}.{PROTOCOL_VERSION_MINOR}
        </p>
      </aside>
    </>
  );
}
