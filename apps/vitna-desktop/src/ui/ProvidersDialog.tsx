import { useEffect, useRef, useState } from 'react';
import type { CatalogModel, CatalogProvider } from '../catalog/catalog';
import { CATALOG, findProvider, formatRate, formatTokens } from '../catalog/catalog';
import type { HostCapabilities, HostProvider } from '../transport/types';
import { ChevronDownIcon, ChevronRightIcon, CloseIcon, SearchIcon } from './icons';

interface ProvidersDialogProps {
  host: HostCapabilities;
  onClose(): void;
}

/**
 * The model providers the host can reach. This window never asks for a key:
 * Vitna's Honesty Contract keeps credentials with the customer, resolved by
 * the host from the OS keychain, workload identity or a helper, so "Connect"
 * hands the whole flow to the host.
 *
 * What each provider sells comes from the pinned catalog (src/catalog/), which
 * is a copy of models.dev taken on a date and committed. The host is asked
 * about credentials and nothing else, so this list is the same whether or not
 * a daemon is running, and the prices are the prices as of the date in the
 * footer rather than whatever a server would say today.
 */
export function ProvidersDialog({ host, onClose }: ProvidersDialogProps) {
  const [providers, setProviders] = useState<HostProvider[] | null>(null);
  const [query, setQuery] = useState('');
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState('');
  const [opened, setOpened] = useState<Record<string, boolean>>({});
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
  const modelMatches = (model: CatalogModel) =>
    model.sku.toLowerCase().includes(needle) || model.name.toLowerCase().includes(needle);

  // A search reaches the models too, so typing "opus" finds Anthropic rather
  // than nothing. A provider whose own name matches keeps its whole list.
  const shown = (providers ?? []).filter((p) => {
    if (!needle) return true;
    if (p.name.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle)) return true;
    return findProvider(p.id)?.models.some(modelMatches) ?? false;
  });
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

  /** "no per-token price (4), no context limit (1)", grouped so a long list reads as a sentence. */
  const omissionLine = (entry: CatalogProvider): string => {
    const byReason = new Map<string, number>();
    for (const omitted of entry.omitted_skus) byReason.set(omitted.reason, (byReason.get(omitted.reason) ?? 0) + 1);
    const parts = [...byReason].map(([reason, count]) => `${reason} (${count})`);
    return `Not listed here: ${parts.join(', ')}.`;
  };

  const row = (provider: HostProvider) => {
    const entry = findProvider(provider.id);
    const all = entry?.models ?? [];
    const hits = needle ? all.filter(modelMatches) : [];
    const listed = needle && hits.length > 0 ? hits : all;
    const expanded = opened[provider.id] ?? (needle !== '' && hits.length > 0);

    return (
      <li key={provider.id} className="provider-item">
        <div className="provider-row">
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
        </div>

        {entry ? (
          <button
            type="button"
            className="model-toggle"
            aria-expanded={expanded}
            // Updated from the current map rather than the one this render
            // closed over: two toggles flipped in one tick both land, instead
            // of the second spreading a stale copy over the first.
            onClick={() => setOpened((current) => ({ ...current, [provider.id]: !expanded }))}
          >
            {expanded ? <ChevronDownIcon /> : <ChevronRightIcon />}
            {needle && hits.length > 0 && hits.length !== all.length
              ? `${hits.length} of ${all.length} models`
              : `${all.length} models`}
          </button>
        ) : (
          // A provider the host offers and the snapshot does not carry. Saying
          // so is the point: an empty list under a name reads as a provider
          // with nothing to sell, which is a different and untrue statement.
          <p className="model-note">This provider is not in the pinned catalog.</p>
        )}

        {entry && expanded ? (
          <ul className="model-list">
            {listed.map((model) => (
              <li key={model.sku} className="model-row">
                <span className="model-sku mono">{model.sku}</span>
                {model.tool_call ? null : <span className="model-flag">no tools</span>}
                <span className="model-facts mono">
                  {formatTokens(model.context_tokens)} ctx · {formatRate(model)}
                </span>
              </li>
            ))}
            {entry.omitted_skus.length > 0 ? <li className="model-note">{omissionLine(entry)}</li> : null}
          </ul>
        ) : null}
      </li>
    );
  };

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
          <br />
          Prices are US dollars per million tokens, from a pinned copy of {hostOf(CATALOG.source)} taken{' '}
          {CATALOG.fetched_at}. This window never fetches them, and the daemon, not this page, says what a run cost.
        </footer>
      </div>
    </div>
  );
}

/**
 * "models.dev" out of the snapshot's own source URL. Derived rather than typed
 * again, so a catalog re-pointed at another source cannot leave this footer
 * naming the old one.
 */
function hostOf(source: string): string {
  try {
    return new URL(source).host;
  } catch {
    return source;
  }
}
