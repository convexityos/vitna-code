import { useMemo, useState } from 'react';
import type { SessionActions } from '../client/useSession';
import type { DiffFile } from '../run/diff';
import type { ApprovalView, ToolCallView } from '../run/model';
import { ApprovalBox } from './ApprovalBox';
import { ChevronRightIcon } from './icons';
import { decodeChunks, decodeText, formatBytes, formatDuration, prettyJson, shortDigest, showControls } from './text';
import { argumentSummary, effectWord, toolTitle } from './words';

interface ToolRowProps {
  tool: ToolCallView;
  approvals: ApprovalView[];
  diff: DiffFile | null;
  connected: boolean;
  actions: SessionActions;
}

type Phase = 'proposed' | 'waiting' | 'running' | 'done' | 'failed' | 'skipped';

function phaseOf(tool: ToolCallView, approvals: ApprovalView[]): Phase {
  const f = tool.finished;
  if (f) {
    if (f.status === 'cancelled') return tool.startedAtMs === null ? 'skipped' : 'failed';
    if (f.status === 'failed' || f.exit_code !== 0) return 'failed';
    return 'done';
  }
  if (tool.startedAtMs !== null) return 'running';
  if (approvals.some((a) => a.status === 'pending' || a.status === 'send_failed')) return 'waiting';
  return 'proposed';
}

const GLYPH: Record<Phase, string> = {
  proposed: '○',
  waiting: '○',
  running: '◐',
  done: '●',
  failed: '●',
  skipped: '○',
};

function statusText(tool: ToolCallView, phase: Phase): string {
  const f = tool.finished;
  switch (phase) {
    case 'proposed':
      return 'proposed';
    case 'waiting':
      return 'waiting on you';
    case 'running':
      return 'running';
    case 'skipped':
      return 'did not run';
    case 'failed':
      return f ? `${f.status === 'cancelled' ? 'cancelled' : `exit ${f.exit_code}`} · ${formatDuration(f.duration_ms)}` : 'failed';
    case 'done':
      return f ? `exit ${f.exit_code} · ${formatDuration(f.duration_ms)}` : 'done';
  }
}

/**
 * One tool call, one line: glyph, verb, target, status. Its output tucks
 * under it behind a `⎿`, collapsed to a first line and a count, and opens on
 * a click. An approval it is waiting on sits between the two, in the flow.
 */
export function ToolRow({ tool, approvals, diff, connected, actions }: ToolRowProps) {
  const [open, setOpen] = useState(false);
  const proposed = tool.proposed;
  const name = proposed?.tool_name ?? 'unnamed tool';
  const argsText = useMemo(() => (proposed ? decodeText(proposed.raw_arguments_json) : ''), [proposed]);
  const target = proposed ? argumentSummary(name, argsText) : null;
  const phase = phaseOf(tool, approvals);
  const output = useMemo(
    () => tool.segments.map((s) => ({ stream: s.stream, text: showControls(decodeChunks(s.chunks)) })),
    [tool.segments],
  );
  const allText = output.map((s) => s.text).join('');
  const lines = allText.split('\n').filter((l) => l.trim() !== '');
  const firstLine = lines[0] ?? '';
  const hasOutput = tool.keptBytes > 0 || tool.droppedBytes > 0;
  const effect = proposed?.effect_class ?? '';

  return (
    <li className={`tool phase-${phase}`}>
      <button
        type="button"
        className="tool-line"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        title={proposed ? `${name} ${proposed.tool_version}` : undefined}
      >
        <span className={`gutter ${phase === 'failed' || phase === 'waiting' ? 'tone-rust' : phase === 'proposed' || phase === 'skipped' ? 'fainter' : ''}`} aria-hidden="true">
          {GLYPH[phase]}
        </span>
        <span className="tool-verb">{toolTitle(name)}</span>
        {target ? <span className="tool-target">{target}</span> : <span className="tool-target fainter">{name}</span>}
        {effect && effect !== 'read' ? <span className="tool-effect">{effectWord(effect)}</span> : null}
        <span className={`tool-status ${phase === 'failed' || phase === 'waiting' ? 'tone-rust' : phase === 'done' || phase === 'running' ? 'faint' : 'fainter'}`}>
          {statusText(tool, phase)}
        </span>
        <ChevronRightIcon className={`icon tool-chevron${open ? ' is-open' : ''}`} />
      </button>

      {approvals.map((approval) => (
        <div key={approval.request.approval_id} className="tool-under">
          <span className="gutter" aria-hidden="true" />
          <ApprovalBox approval={approval} tool={tool} diff={diff} connected={connected} actions={actions} />
        </div>
      ))}

      {hasOutput || open ? (
        <div className="tool-under">
          <span className="gutter fainter" aria-hidden="true">
            {'⎿'}
          </span>
          {open ? (
            <div className="tool-detail">
              {hasOutput ? (
                <pre className="tool-pre">
                  {output.map((segment, i) => (
                    <span key={i} className={segment.stream === 'stderr' ? 'stream-err' : undefined}>
                      {segment.text}
                    </span>
                  ))}
                  {tool.droppedBytes > 0 ? (
                    <span className="fainter">{`\n… ${formatBytes(tool.droppedBytes)} more not kept in this window`}</span>
                  ) : null}
                </pre>
              ) : (
                <p className="mono fainter">no output</p>
              )}
              {proposed && argsText ? (
                <details className="tool-args">
                  <summary className="mono fainter">
                    arguments · sha256 {shortDigest(proposed.argument_digest)} · definition {shortDigest(proposed.definition_digest)}
                  </summary>
                  <pre className="tool-pre">{showControls(prettyJson(argsText))}</pre>
                </details>
              ) : null}
              {tool.finished && (tool.finished.stdout_digest || tool.finished.stderr_digest) ? (
                <p className="mono fainter">
                  stdout {shortDigest(tool.finished.stdout_digest)} · stderr {shortDigest(tool.finished.stderr_digest)}
                </p>
              ) : null}
            </div>
          ) : (
            <button type="button" className="tool-summary" onClick={() => setOpen(true)}>
              <span className="tool-first">{firstLine || (tool.droppedBytes ? 'output not kept' : 'no output')}</span>
              {lines.length > 1 ? <span className="fainter"> · {lines.length} lines</span> : null}
            </button>
          )}
        </div>
      ) : null}
    </li>
  );
}
