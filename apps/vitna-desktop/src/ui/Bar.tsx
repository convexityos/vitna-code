import type { ViewName } from '../app/Workbench';
import type { LinkState } from '../transport/types';
import { ChangesIcon, CloseIcon, MenuIcon, NewSessionIcon } from './icons';
import { shortDigest } from './text';

interface BarProps {
  sessionTitle: string;
  isSample: boolean;
  link: LinkState;
  activeView: ViewName;
  receiptOpen: boolean;
  onView(view: ViewName): void;
  onCloseReceipt(): void;
  drawerOpen: boolean;
  onToggleDrawer(): void;
  onNewSession: (() => Promise<void>) | null;
  changesCount: number;
  changesOpen: boolean;
  onToggleChanges(): void;
}

export function Bar(props: BarProps) {
  const { activeView } = props;
  return (
    <header className="bar">
      <div className="bar-start">
        <button
          type="button"
          className="pill icon-pill pill-small bar-quiet"
          aria-label="Session and views"
          aria-expanded={props.drawerOpen}
          onClick={props.onToggleDrawer}
        >
          <MenuIcon />
        </button>
        <button
          type="button"
          className="pill pill-small bar-quiet"
          disabled={!props.onNewSession}
          title={props.onNewSession ? undefined : 'This host does not open sessions from this window'}
          onClick={() => void props.onNewSession?.()}
        >
          <NewSessionIcon />
          New session
        </button>
      </div>

      <div className="tabs" role="tablist" aria-label="Open views">
        <button
          type="button"
          role="tab"
          className="tab"
          aria-selected={activeView === 'session'}
          onClick={() => props.onView('session')}
        >
          <span className="tab-title">{props.sessionTitle}</span>
          {props.isSample ? <span className="tab-tag">sample</span> : null}
        </button>
        {props.receiptOpen ? (
          <span className="tab-pair">
            <button
              type="button"
              role="tab"
              className="tab"
              aria-selected={activeView === 'receipt'}
              onClick={() => props.onView('receipt')}
            >
              <span className="tab-title">Receipt</span>
            </button>
            <button type="button" className="tab-close" aria-label="Close the receipt reader" onClick={props.onCloseReceipt}>
              <CloseIcon />
            </button>
          </span>
        ) : null}
      </div>

      <div className="bar-end">
        {props.changesCount > 0 && activeView === 'session' ? (
          <button
            type="button"
            className="pill pill-small bar-quiet"
            aria-pressed={props.changesOpen}
            onClick={props.onToggleChanges}
          >
            <ChangesIcon />
            Changes
          </button>
        ) : null}
        <LinkWord link={props.link} isSample={props.isSample} />
      </div>
    </header>
  );
}

function LinkWord({ link, isSample }: { link: LinkState; isSample: boolean }) {
  switch (link.status) {
    case 'connected':
      if (isSample) {
        return (
          <span className="link-word tone-rust" title={link.daemonBuildCommit} role="status">
            Scripted sample, nothing runs
          </span>
        );
      }
      return (
        <span className="link-word" role="status" title={`vitna-coded ${link.daemonBuildCommit}, protocol ${link.protocol}`}>
          <span className="ok-dot" aria-hidden="true" />
          Connected
          <span className="link-build">{shortDigest(link.daemonBuildCommit, 9)}</span>
        </span>
      );
    case 'no_bridge':
      return (
        <span className="link-word faint" role="status">
          No daemon
        </span>
      );
    case 'idle':
    case 'connecting':
      return (
        <span className="link-word faint" role="status">
          Connecting
        </span>
      );
    case 'refused':
      return (
        <span className="link-word tone-rust" role="status" title={link.reason}>
          Refused by the daemon
        </span>
      );
    case 'lost':
      return (
        <span className="link-word tone-rust" role="status" title={link.reason}>
          Connection lost
        </span>
      );
  }
}
