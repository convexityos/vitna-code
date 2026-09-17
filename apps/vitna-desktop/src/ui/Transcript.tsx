import { useEffect, useMemo, useRef, type ReactNode } from 'react';
import type { SessionActions } from '../client/useSession';
import { TERMINAL_RUN_STATES } from '../protocol/types';
import { parseUnifiedDiff, type DiffFile } from '../run/diff';
import type { SessionView, TimelineItem, ToolCallView } from '../run/model';
import { ApprovalBox } from './ApprovalBox';
import { PlanList } from './PlanList';
import { decodeText, formatElapsed } from './text';
import { ToolRow } from './ToolRow';
import { useNow } from './useNow';
import { argumentSummary, diagnosticTone, runStateWord } from './words';

interface TranscriptProps {
  view: SessionView;
  connected: boolean;
  actions: SessionActions;
  composer: ReactNode;
}

/**
 * The session as a transcript: one column, a glyph gutter down its left, and
 * the prompt docked under it. Every entry hangs off the gutter, which is what
 * makes a screen read as a terminal rather than a page of cards: the model
 * speaks after a dot, you speak after a prompt glyph, a tool is one line with
 * its result tucked under it, and a decision is a box in the flow with
 * numbered choices.
 */
export function Transcript({ view, connected, actions, composer }: TranscriptProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const run = view.currentRunId ? view.runs[view.currentRunId] : undefined;
  const runActive = Boolean(run?.state && !TERMINAL_RUN_STATES.has(run.state));
  const now = useNow(runActive);

  // Text the daemon echoed back as a user message is shown once, where the daemon put it.
  const echoed = useMemo(
    () => new Set(Object.values(view.messages).filter((m) => m.role === 'user').map((m) => m.text.trim())),
    [view.messages],
  );

  // The workspace diff, parsed once per digest, so an approval to write a file can show its hunks.
  const diffFiles = useMemo(() => {
    const files = new Map<string, DiffFile>();
    for (const id of view.diffOrder) {
      const diff = view.diffs[id];
      if (!diff) continue;
      for (const file of parseUnifiedDiff(diff.diff_text).files) files.set(file.path, file);
    }
    return files;
  }, [view.diffOrder, view.diffs]);

  // Follow the stream while the reader is at the bottom; stop once they scroll up.
  useEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  });

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  const lastModelMessage = useMemo(() => {
    for (let i = view.timeline.length - 1; i >= 0; i -= 1) {
      const item = view.timeline[i];
      if (!item) continue;
      if (item.type === 'message') {
        const m = view.messages[item.id];
        return m && m.role !== 'user' ? m.id : null;
      }
      if (item.type !== 'reasoning') return null;
    }
    return null;
  }, [view.timeline, view.messages]);

  const streamingId = runActive && run?.state === 'running_model' ? lastModelMessage : null;
  const word = runStateWord(run?.state);

  return (
    <div className="transcript">
      <div className="transcript-scroll" ref={scroller} onScroll={onScroll}>
        <ol className="transcript-column" aria-label="Session transcript">
          {view.timeline.map((item) => (
            <Entry
              key={`${item.type}:${item.id}`}
              item={item}
              view={view}
              echoed={echoed}
              diffFiles={diffFiles}
              connected={connected}
              actions={actions}
              streaming={item.type === 'message' && item.id === streamingId}
            />
          ))}
          {view.gap ? (
            <li className="t-line t-note" role="status">
              <span className="gutter tone-rust">!</span>
              <span>Events after #{view.gap.after} were missed. Asking the daemon to send them again.</span>
            </li>
          ) : null}
          {runActive && run ? (
            <li className="t-line t-working" aria-live="polite">
              <span className={`gutter ${word.tone === 'rust' ? 'tone-rust' : 'tone-peri'}`}>
                {run.state === 'waiting_for_approval' ? '?' : '✦'}
              </span>
              <span className="t-working-text">
                <span className={word.tone === 'rust' ? 'tone-rust' : ''}>{word.word.toLowerCase()}</span>
                <span className="fainter">
                  {run.reason ? ` · ${run.reason}` : ''} · {formatElapsed(now - run.firstSeenAt)}
                </span>
              </span>
            </li>
          ) : null}
        </ol>
      </div>
      <div className="dock">
        <div className="dock-inner">{composer}</div>
      </div>
    </div>
  );
}

interface EntryProps {
  item: TimelineItem;
  view: SessionView;
  echoed: Set<string>;
  diffFiles: Map<string, DiffFile>;
  connected: boolean;
  actions: SessionActions;
  streaming: boolean;
}

function diffForTool(tool: ToolCallView | null, diffFiles: Map<string, DiffFile>): DiffFile | null {
  if (!tool?.proposed) return null;
  const effect = tool.proposed.effect_class;
  if (effect !== 'write' && effect !== '') return null;
  const path = argumentSummary(tool.proposed.tool_name, decodeText(tool.proposed.raw_arguments_json));
  return (path && diffFiles.get(path)) || null;
}

function Entry({ item, view, echoed, diffFiles, connected, actions, streaming }: EntryProps) {
  switch (item.type) {
    case 'run': {
      const run = view.runs[item.id];
      const word = runStateWord(run?.state);
      const ended = Boolean(run?.state && TERMINAL_RUN_STATES.has(run.state));
      return (
        <li className="t-rule" aria-label={`Run ${item.id}: ${word.word}`}>
          <span className="t-rule-text">
            {item.id} · <span className={word.tone === 'rust' ? 'tone-rust' : ''}>{word.word.toLowerCase()}</span>
            {ended && run ? ` · ${formatElapsed(run.changedAt - run.firstSeenAt)}` : ''}
            {ended && run?.reason ? ` · ${run.reason}` : ''}
          </span>
        </li>
      );
    }
    case 'plan': {
      if (!view.plan || view.plan.plan_id !== item.id) return null;
      const run = view.currentRunId ? view.runs[view.currentRunId] : undefined;
      return (
        <li className="t-line">
          <span className="gutter fainter" aria-hidden="true">
            {'≡'}
          </span>
          <PlanList plan={view.plan} waiting={run?.state === 'waiting_for_approval'} finished={run?.state === 'completed'} />
        </li>
      );
    }
    case 'turn': {
      const turn = view.turns[item.id];
      if (!turn || echoed.has(turn.prompt.trim())) return null;
      return (
        <li className="t-you">
          <span className="gutter tone-peri" aria-hidden="true">
            {'❯'}
          </span>
          <div className="t-you-body">
            <p className="t-you-text">{turn.prompt}</p>
            <p className="mono fainter">sent · not yet echoed by the daemon</p>
          </div>
        </li>
      );
    }
    case 'message': {
      const message = view.messages[item.id];
      if (!message) return null;
      if (message.role === 'user') {
        return (
          <li className="t-you">
            <span className="gutter tone-peri" aria-hidden="true">
              {'❯'}
            </span>
            <div className="t-you-body">
              <p className="t-you-text">{message.text}</p>
            </div>
          </li>
        );
      }
      return (
        <li className="t-line t-model" data-role={message.role || 'unstated'}>
          <span className="gutter" aria-hidden="true">
            {'●'}
          </span>
          <div>
            {message.role !== 'model' ? <p className="mono fainter">{message.role || 'role not stated'}</p> : null}
            <Prose text={message.text} streaming={streaming} />
          </div>
        </li>
      );
    }
    case 'reasoning': {
      const reasoning = view.reasoning[item.id];
      if (!reasoning) return null;
      // One line, the way every reference collapses it; the whole summary a click away.
      const firstLine = reasoning.text.split(/(?<=[.!?])\s|\n/)[0] ?? reasoning.text;
      const short = firstLine.length > 96 ? `${firstLine.slice(0, 95)}…` : firstLine;
      const more = short !== reasoning.text;
      return (
        <li className="t-line t-reasoning">
          <span className="gutter fainter" aria-hidden="true">
            {'∴'}
          </span>
          {more ? (
            <details className="reasoning">
              <summary>
                <span className="fainter">reasoning</span> {short}
              </summary>
              <p>{reasoning.text}</p>
            </details>
          ) : (
            <p>
              <span className="fainter">reasoning</span> {reasoning.text}
            </p>
          )}
        </li>
      );
    }
    case 'tool': {
      const tool = view.tools[item.id];
      if (!tool) return null;
      return (
        <ToolRow
          tool={tool}
          approvals={tool.approvalIds.map((id) => view.approvals[id]).filter((a) => a !== undefined)}
          diff={diffForTool(tool, diffFiles)}
          connected={connected}
          actions={actions}
        />
      );
    }
    case 'approval': {
      const approval = view.approvals[item.id];
      if (!approval) return null;
      return (
        <li className="t-line">
          <span className="gutter tone-rust" aria-hidden="true">
            ?
          </span>
          <ApprovalBox approval={approval} tool={null} diff={null} connected={connected} actions={actions} />
        </li>
      );
    }
    case 'child': {
      const child = view.children[item.id];
      if (!child) return null;
      return (
        <li className="t-line t-child">
          <span className="gutter fainter" aria-hidden="true">
            {'↳'}
          </span>
          <div>
            <p>{child.spawned?.goal || 'Sub-agent, goal not stated'}</p>
            <p className="mono fainter">
              {item.id}
              {child.spawned?.agent_workspace_path ? ` · ${child.spawned.agent_workspace_path}` : ''}
              {' · '}
              {child.completed ? child.completed.completion_state || 'finished, state not stated' : 'working'}
            </p>
          </div>
        </li>
      );
    }
    case 'diagnostic': {
      const d = view.diagnostics[item.id];
      if (!d) return null;
      const tone = diagnosticTone(d.level);
      return (
        <li className="t-line t-diag" data-level={d.level}>
          <span className={`gutter ${tone === 'rust' ? 'tone-rust' : 'fainter'}`} aria-hidden="true">
            {tone === 'rust' ? '!' : 'i'}
          </span>
          <p className="mono">
            <span className={tone === 'rust' ? 'tone-rust' : 'faint'}>{d.level || 'note'}</span>
            {'  '}
            <span className="t-diag-text">{d.message}</span>
            {d.origin === 'client' ? <span className="fainter"> (this window)</span> : null}
            {d.code || d.details ? (
              <span className="fainter t-diag-code">
                {'\n'}
                {[d.code, d.details].filter(Boolean).join(' · ')}
              </span>
            ) : null}
          </p>
        </li>
      );
    }
    case 'unknown': {
      const u = view.unknown[item.id];
      if (!u) return null;
      return (
        <li className="t-line t-diag" data-level="warn">
          <span className="gutter tone-rust" aria-hidden="true">
            ?
          </span>
          <p className="mono">
            <span className="tone-rust">unread</span>
            {'  '}An event this window could not read ({u.reason.replace('_', ' ')}).
            <span className="fainter t-diag-code">
              {'\n'}
              {u.detail}
            </span>
          </p>
        </li>
      );
    }
  }
}

/** Prose with `code` spans and, while it streams, a caret at its end. Nothing else is interpreted. */
function Prose({ text, streaming }: { text: string; streaming: boolean }) {
  const parts = text.split(/(`[^`\n]+`)/g);
  return (
    <p className="prose">
      {parts.map((part, i) =>
        part.startsWith('`') && part.endsWith('`') && part.length > 2 ? (
          <code key={i}>{part.slice(1, -1)}</code>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
      {streaming ? <span className="caret" aria-hidden="true" /> : null}
    </p>
  );
}
