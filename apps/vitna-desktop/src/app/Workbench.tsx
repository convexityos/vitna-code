import { useCallback, useMemo, useState } from 'react';
import { useSession } from '../client/useSession';
import { TERMINAL_RUN_STATES } from '../protocol/types';
import type { Transport } from '../transport/types';
import { Bar } from '../ui/Bar';
import { ChangesPanel } from '../ui/ChangesPanel';
import { Composer } from '../ui/Composer';
import type { ComposerIntent } from '../ui/composerInput';
import { Drawer } from '../ui/Drawer';
import { EmptyStage } from '../ui/EmptyStage';
import { ProvidersDialog } from '../ui/ProvidersDialog';
import { ReceiptView } from '../ui/ReceiptView';
import { Transcript } from '../ui/Transcript';

export type ViewName = 'session' | 'receipt';

interface WorkbenchProps {
  transport: Transport | null;
  initialPrompt: string;
  onOpenSample: (() => void) | null;
  sampleError: string;
}

export interface Notice {
  tone: 'plain' | 'rust';
  text: string;
}

export function Workbench({ transport, initialPrompt, onOpenSample, sampleError }: WorkbenchProps) {
  const { kind, link, view, session, host, actions } = useSession(transport);
  const [activeView, setActiveView] = useState<ViewName>('session');
  const [receiptOpen, setReceiptOpen] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [providersOpen, setProvidersOpen] = useState(false);
  const [changesOpen, setChangesOpen] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);

  const isSample = kind === 'sample';
  const connected = link.status === 'connected';
  const currentRun = view.currentRunId ? view.runs[view.currentRunId] ?? null : null;
  const runActive = Boolean(currentRun?.state && !TERMINAL_RUN_STATES.has(currentRun.state));
  const usage = view.currentRunId ? view.usage[view.currentRunId] ?? null : null;
  const hasActivity = view.timeline.length > 0;
  const diffCount = view.diffOrder.length;

  const openReceipt = useCallback(() => {
    setReceiptOpen(true);
    setActiveView('receipt');
    setDrawerOpen(false);
  }, []);

  const closeReceipt = useCallback(() => {
    setReceiptOpen(false);
    setActiveView('session');
  }, []);

  const openProviders = useCallback(() => {
    setProvidersOpen(true);
    setDrawerOpen(false);
  }, []);

  const disabledReason = useMemo((): string | null => {
    switch (link.status) {
      case 'no_bridge':
        return 'No daemon is connected to this window.';
      case 'idle':
      case 'connecting':
        return 'Connecting to the daemon.';
      case 'refused':
        return `The daemon refused this window: ${link.reason}`;
      case 'lost':
        return `The connection to the daemon was lost: ${link.reason}`;
      case 'connected':
        return session ? null : 'The host did not open this window for a session.';
    }
  }, [link, session]);

  /** Acts on what the composer holds. Returns true when the text was used and can be cleared. */
  const onIntent = useCallback(
    async (intent: ComposerIntent): Promise<boolean> => {
      setNotice(null);
      switch (intent.kind) {
        case 'empty':
          return false;
        case 'unknown_command':
          setNotice({
            tone: 'rust',
            text: `There is no /${intent.name} command. The commands are /steer, /pause, /resume, /cancel, /receipt and /providers.`,
          });
          return false;
        case 'prompt': {
          if (runActive) {
            setNotice({ tone: 'rust', text: 'A run is going. Use /steer to correct it, or /cancel to stop it first.' });
            return false;
          }
          const sent = await actions.submitTurn(intent.prompt, intent.attachments);
          if (!sent) setNotice({ tone: 'rust', text: 'The prompt was not sent. The reason is in the session.' });
          return sent;
        }
        case 'command': {
          if (intent.name === 'receipt') {
            openReceipt();
            return true;
          }
          if (intent.name === 'providers') {
            openProviders();
            return true;
          }
          if (!runActive) {
            setNotice({ tone: 'plain', text: `There is no run going, so /${intent.name} has nothing to act on.` });
            return false;
          }
          if (intent.name === 'steer') {
            if (!intent.argument) {
              setNotice({ tone: 'plain', text: 'Write the correction after /steer.' });
              return false;
            }
            return actions.steer(intent.argument);
          }
          if (intent.name === 'pause') return actions.pause();
          if (intent.name === 'resume') return actions.resume();
          return actions.cancel();
        }
      }
    },
    [actions, openProviders, openReceipt, runActive],
  );

  const composer = (variant: 'hero' | 'dock') => (
    <Composer
      variant={variant}
      // Only the opening composer is prefilled; the docked one starts empty.
      initialText={variant === 'hero' ? initialPrompt : ''}
      disabledReason={disabledReason}
      mode={session?.mode ?? ''}
      usage={usage}
      run={currentRun}
      runActive={runActive}
      pickFiles={host.pickFiles ?? null}
      notice={notice}
      onIntent={onIntent}
      onPause={() => void actions.pause()}
      onResume={() => void actions.resume()}
      onCancel={() => void actions.cancel()}
    />
  );

  return (
    <div className="app" data-drawer={drawerOpen ? 'open' : 'closed'}>
      <Bar
        sessionTitle={session?.title ?? (link.status === 'no_bridge' ? 'No session' : 'Session')}
        isSample={isSample}
        link={link}
        activeView={activeView}
        receiptOpen={receiptOpen}
        onView={setActiveView}
        onCloseReceipt={closeReceipt}
        drawerOpen={drawerOpen}
        onToggleDrawer={() => setDrawerOpen((open) => !open)}
        onNewSession={host.newSession ?? null}
        changesCount={diffCount}
        changesOpen={changesOpen}
        onToggleChanges={() => setChangesOpen((open) => !open)}
      />
      <div className="body">
        {drawerOpen ? (
          <Drawer
            session={session}
            link={link}
            isSample={isSample}
            activeView={activeView}
            onView={(next) => {
              if (next === 'receipt') openReceipt();
              else setActiveView(next);
              setDrawerOpen(false);
            }}
            onProviders={openProviders}
            onClose={() => setDrawerOpen(false)}
          />
        ) : null}
        <main
          className="stage"
          data-view={activeView}
          data-changes={activeView === 'session' && diffCount > 0 && changesOpen ? 'open' : 'closed'}
        >
          {activeView === 'receipt' ? (
            <ReceiptView isSample={isSample} />
          ) : hasActivity ? (
            <Transcript view={view} connected={connected} actions={actions} composer={composer('dock')} />
          ) : (
            <EmptyStage
              link={link}
              session={session}
              host={host}
              composer={composer('hero')}
              onOpenSample={onOpenSample}
              sampleError={sampleError}
              onProviders={openProviders}
            />
          )}
          {activeView === 'session' && diffCount > 0 && changesOpen ? (
            <ChangesPanel diffs={view.diffOrder.map((id) => view.diffs[id]).filter((d) => d !== undefined)} />
          ) : null}
        </main>
      </div>
      {providersOpen ? <ProvidersDialog host={host} onClose={() => setProvidersOpen(false)} /> : null}
    </div>
  );
}
