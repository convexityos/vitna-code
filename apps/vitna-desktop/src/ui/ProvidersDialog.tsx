import { useEffect, useRef, useState } from 'react';
import type { HostCapabilities, HostProvider } from '../transport/types';
import { CloseIcon, SearchIcon } from './icons';

interface ProvidersDialogProps {
  host: HostCapabilities;
  onClose(): void;
}

/**
 * The model providers the host can reach. This window never asks for a key:
 * Vitna's Honesty Contract keeps credentials with the customer, resolved by
 * the host from the OS keychain, workload identity or a helper, so "Connect"
 * hands the whole flow to the host.
 */
export function ProvidersDialog({ host, onClose }: ProvidersDialogProps) {
  const [providers, setProviders] = useState<HostProvider[] | null>(null);
  const [query, setQuery] = useState('');
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState('');
  const dialog = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const list = host.listProviders;

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    search.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
      if (event.key === 'Tab' && dialog.current) {
        const focusable = dialog.current.querySelectorAll<HTMLElement>('button:not(:disabled), input');
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (!first || !last) return;
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      previous?.focus();
    };
  }, [onClose]);

  useEffect(() => {
    if (!list) return;
    list()
      .then(setProviders)
      .catch((e: unknown) => {
        setProviders([]);
        setMessage(e instanceof Error ? e.message : String(e));
      });
  }, [list]);

  const needle = query.trim().toLowerCase();
  const shown = (providers ?? []).filter(
    (p) => !needle || p.name.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle),
  );
  const ready = shown.filter((p) => p.credentialSource !== null);
  const notReady = shown.filter((p) => p.credentialSource === null);

  const connect = async (provider: HostProvider) => {
    if (!host.connectProvider) return;
    setBusy(provider.id);
    setMessage('');
    try {
      await host.connectProvider(provider.id);
      if (list) setProviders(await list());
    } catch (e) {
      setMessage(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy('');
    }
  };

  const row = (provider: HostProvider) => (
    <li key={provider.id} className="provider-row">
      <span className="provider-mark" aria-hidden="true">
        {provider.name.slice(0, 1).toUpperCase()}
      </span>
      <span className="provider-text">
        <span className="provider-name">{provider.name}</span>
        <span className="provider-desc">{provider.description}</span>
      </span>
      {provider.credentialSource !== null ? (
        <span className="provider-source mono">
          <span className="ok-dot" aria-hidden="true" />
          {provider.credentialSource}
        </span>
      ) : host.connectProvider ? (
        <button
          type="button"
          className="pill pill-small"
          disabled={busy === provider.id}
          onClick={() => void connect(provider)}
        >
          {busy === provider.id ? 'Waiting on the host' : 'Connect'}
        </button>
      ) : (
        <span className="provider-source mono fainter">not set up</span>
      )}
    </li>
  );

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="providers-title" ref={dialog}>
        <header className="dialog-head">
          <h2 id="providers-title" className="dialog-title">
            Model providers
          </h2>
          <button type="button" className="pill icon-pill pill-small" aria-label="Close" onClick={onClose}>
            <CloseIcon />
          </button>
        </header>

        {list ? (
          <>
            <label className="popover-search dialog-search">
              <SearchIcon />
              <span className="visually-hidden">Search providers</span>
              <input
                ref={search}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search providers"
              />
            </label>
            <div className="dialog-body">
              {providers === null ? <p className="dialog-note">Asking the host.</p> : null}
              {ready.length > 0 ? (
                <section>
                  <h3 className="dialog-group">Ready</h3>
                  <ul>{ready.map(row)}</ul>
                </section>
              ) : null}
              {notReady.length > 0 ? (
                <section>
                  <h3 className="dialog-group">Not set up</h3>
                  <ul>{notReady.map(row)}</ul>
                </section>
              ) : null}
              {providers !== null && shown.length === 0 ? (
                <p className="dialog-note">{needle ? 'No provider matches.' : 'The host lists no providers.'}</p>
              ) : null}
              {message ? <p className="dialog-note tone-rust">{message}</p> : null}
            </div>
          </>
        ) : (
          <div className="dialog-body">
            <p className="dialog-note">
              This host does not list its providers. Set them up where the daemon runs; this window never handles a key.
            </p>
          </div>
        )}

        <footer className="dialog-foot">
          Keys stay with the host. {host.connectProvider ? 'Connect starts the host’s own sign-in.' : 'This host sets providers up outside this window.'}
        </footer>
      </div>
    </div>
  );
}
