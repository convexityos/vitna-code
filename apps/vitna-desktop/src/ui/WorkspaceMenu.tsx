import { useEffect, useId, useRef, useState } from 'react';
import type { HostCapabilities, HostSession, HostWorkspace } from '../transport/types';
import { CheckIcon, ChevronDownIcon, FolderIcon, PlusIcon, SearchIcon } from './icons';
import { basename } from './text';

interface WorkspaceMenuProps {
  session: HostSession | null;
  host: HostCapabilities;
}

/**
 * The workspace this session is bound to, and the others the host knows.
 * Switching is the host's job: a session is created against one workspace
 * and stays there, so choosing another asks the host to open it.
 */
export function WorkspaceMenu({ session, host }: WorkspaceMenuProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<HostWorkspace[] | null>(null);
  const [error, setError] = useState('');
  const root = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const listId = useId();

  useEffect(() => {
    if (!open) return undefined;
    search.current?.focus();
    const onDown = (event: MouseEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  useEffect(() => {
    if (!open || items !== null || !host.listWorkspaces) return;
    host
      .listWorkspaces()
      .then(setItems)
      .catch((e: unknown) => {
        setItems([]);
        setError(e instanceof Error ? e.message : String(e));
      });
  }, [open, items, host]);

  if (!session) {
    return (
      <span className="workspace-pill is-empty">
        <FolderIcon />
        No workspace
      </span>
    );
  }

  const known = items ?? [];
  const list = known.some((w) => w.path === session.workspacePath)
    ? known
    : [{ path: session.workspacePath, name: basename(session.workspacePath) }, ...known];
  const needle = query.trim().toLowerCase();
  const shown = list.filter((w) => !needle || w.name.toLowerCase().includes(needle) || w.path.toLowerCase().includes(needle));
  const canOpen = Boolean(host.openWorkspace);

  return (
    <div className="workspace" ref={root}>
      <button
        type="button"
        className="pill pill-small workspace-pill"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        title={session.workspacePath}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="workspace-mark" aria-hidden="true">
          {basename(session.workspacePath).slice(0, 1).toUpperCase() || '?'}
        </span>
        {basename(session.workspacePath)}
        <ChevronDownIcon />
      </button>

      {open ? (
        <div className="popover workspace-popover">
          <label className="popover-search">
            <SearchIcon />
            <span className="visually-hidden">Search workspaces</span>
            <input
              ref={search}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search workspaces"
            />
          </label>
          <ul id={listId} role="listbox" aria-label="Workspaces" className="popover-list">
            {shown.map((w) => {
              const current = w.path === session.workspacePath;
              return (
                <li key={w.path} role="option" aria-selected={current}>
                  <button
                    type="button"
                    className="popover-row"
                    disabled={!current && !canOpen}
                    onClick={() => {
                      if (current) setOpen(false);
                      else void host.openWorkspace?.(w.path).then(() => setOpen(false));
                    }}
                  >
                    <span className="workspace-mark" aria-hidden="true">
                      {w.name.slice(0, 1).toUpperCase() || '?'}
                    </span>
                    <span className="popover-row-text">
                      <span>{w.name}</span>
                      <span className="mono fainter">{w.path}</span>
                    </span>
                    {current ? <CheckIcon /> : null}
                  </button>
                </li>
              );
            })}
            {shown.length === 0 ? <li className="popover-empty">No workspace matches.</li> : null}
          </ul>
          <div className="popover-foot">
            {canOpen ? (
              <button type="button" className="popover-row" onClick={() => void host.openWorkspace?.().then(() => setOpen(false))}>
                <PlusIcon />
                Open another workspace
              </button>
            ) : (
              <p className="popover-note">
                {host.listWorkspaces
                  ? 'This host lists workspaces but does not switch them from this window.'
                  : 'This host does not list other workspaces.'}
              </p>
            )}
            {error ? <p className="popover-note tone-rust">{error}</p> : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
