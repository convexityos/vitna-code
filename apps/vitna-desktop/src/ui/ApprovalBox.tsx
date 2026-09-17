import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { isExpired, type SessionActions } from '../client/useSession';
import type { ApprovalScope } from '../protocol/types';
import type { DiffFile } from '../run/diff';
import type { ApprovalView, ToolCallView } from '../run/model';
import { formatCountdown, groupDigest, showControls } from './text';
import { useNow } from './useNow';
import { effectWord } from './words';

interface ApprovalBoxProps {
  approval: ApprovalView;
  tool: ToolCallView | null;
  diff: DiffFile | null;
  connected: boolean;
  actions: Pick<SessionActions, 'approve' | 'reject'>;
}

interface Option {
  key: string;
  label: string;
  hint: string;
  meaning: string;
  act: 'approve' | 'reject' | 'reason';
  scope: ApprovalScope;
  tone: 'plain' | 'rust';
}

const OPTIONS: Option[] = [
  { key: '1', label: 'Approve once', hint: 'y', meaning: '', act: 'approve', scope: 'once', tone: 'plain' },
  {
    key: '2',
    label: 'Approve for this session',
    hint: '',
    meaning: 'This action runs again for the rest of the session without asking.',
    act: 'approve',
    scope: 'session',
    tone: 'rust',
  },
  {
    key: '3',
    label: 'Approve for this project',
    hint: '',
    meaning: 'This action runs in every later session in this project without asking.',
    act: 'approve',
    scope: 'project',
    tone: 'rust',
  },
  { key: '4', label: 'Reject', hint: 'n', meaning: '', act: 'reject', scope: 'once', tone: 'plain' },
  { key: '5', label: 'Reject with a reason', hint: '', meaning: '', act: 'reason', scope: 'once', tone: 'plain' },
];

const DIFF_PREVIEW_LINES = 28;

/**
 * An exact action waiting for a person, as a box in the transcript with
 * numbered choices, the way a terminal asks. The digest is printed in full
 * because the digest is what gets approved; the daemon's description says
 * what it covers, and the diff shows the change when the action is a write.
 *
 * The box takes focus when it appears only if nothing else has it, `y`, `n`,
 * the digits and the arrows work while it has focus, a held key does not
 * repeat, and a decision that has left this window cannot be changed by any
 * later key or click.
 */
export function ApprovalBox({ approval, tool, diff, connected, actions }: ApprovalBoxProps) {
  const { request } = approval;
  const box = useRef<HTMLElement>(null);
  const [cursor, setCursor] = useState(0);
  const [reason, setReason] = useState('');
  const [askReason, setAskReason] = useState(false);
  const [copied, setCopied] = useState(false);
  const titleId = useId();
  const open = approval.status === 'pending' || approval.status === 'send_failed';
  const now = useNow(open && request.timeout_ms > 0);
  const expired = open && isExpired(approval.receivedAt, request.timeout_ms, now);
  const canDecide = open && !expired && connected;
  const groups = useMemo(() => groupDigest(request.action_digest), [request.action_digest]);
  const isSha256 = /^(sha256:)?[0-9a-f]{64}$/i.test(request.action_digest);
  const proposed = tool?.proposed ?? null;

  // Take focus on arrival only when nobody is typing anywhere.
  useEffect(() => {
    if (!open) return;
    const active = document.activeElement;
    const idle =
      !active ||
      active === document.body ||
      (active instanceof HTMLTextAreaElement && active.value.trim() === '');
    if (idle) box.current?.focus({ preventScroll: true });
    // Once, when the box appears.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const choose = (option: Option) => {
    if (!canDecide) return;
    if (option.act === 'approve') void actions.approve(request.approval_id, option.scope);
    else if (option.act === 'reject') void actions.reject(request.approval_id, '');
    else setAskReason(true);
  };

  const sendReason = () => {
    if (canDecide) void actions.reject(request.approval_id, reason.trim());
  };

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if ((event.target as HTMLElement).closest('input, textarea')) return;
    if (event.repeat || event.altKey || event.ctrlKey || event.metaKey) return;
    const key = event.key;
    const byDigit = OPTIONS.find((o) => o.key === key);
    if (byDigit) {
      event.preventDefault();
      setCursor(OPTIONS.indexOf(byDigit));
      choose(byDigit);
      return;
    }
    if (key === 'y' || key === 'Y') {
      event.preventDefault();
      choose(OPTIONS[0] as Option);
    } else if (key === 'n' || key === 'N') {
      event.preventDefault();
      choose(OPTIONS[3] as Option);
    } else if (key === 'ArrowDown' || key === 'j') {
      event.preventDefault();
      setCursor((c) => (c + 1) % OPTIONS.length);
    } else if (key === 'ArrowUp' || key === 'k') {
      event.preventDefault();
      setCursor((c) => (c - 1 + OPTIONS.length) % OPTIONS.length);
    } else if (key === 'Enter') {
      event.preventDefault();
      choose(OPTIONS[cursor] as Option);
    }
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(request.action_digest);
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };

  if (!open) {
    return <Outcome approval={approval} titleId={titleId} groups={groups} />;
  }

  const current = OPTIONS[cursor] as Option;

  return (
    <section
      className="abox"
      data-expired={expired || undefined}
      tabIndex={0}
      ref={box}
      aria-labelledby={titleId}
      onKeyDown={onKeyDown}
    >
      <header className="abox-head">
        <span className="abox-tag tone-rust">{expired ? 'expired' : approval.status === 'send_failed' ? 'not delivered' : 'approval needed'}</span>
        {proposed ? (
          <span className="abox-tool">
            {proposed.tool_name} <span className="fainter">{effectWord(proposed.effect_class)}</span>
          </span>
        ) : (
          <span className="abox-tool fainter">tool call {request.tool_call_id || 'not named'}</span>
        )}
        {request.timeout_ms > 0 && !expired ? (
          <span className="abox-clock" aria-label="Time left to answer">
            {formatCountdown(approval.receivedAt + request.timeout_ms - now)}
          </span>
        ) : null}
      </header>

      <p id={titleId} className="abox-title">
        {request.description || 'The daemon did not describe this action.'}
      </p>

      <dl className="abox-facts">
        <div>
          <dt>digest</dt>
          <dd>
            {request.action_digest ? (
              <span className="abox-digest" aria-label={`Action digest ${request.action_digest}`}>
                {groups.map((g, i) => (
                  <span key={i}>{g}</span>
                ))}
                {!isSha256 ? <span className="tone-rust"> not a SHA-256</span> : null}
                <button type="button" className="abox-copy" onClick={() => void copy()}>
                  {copied ? 'copied' : 'copy'}
                </button>
              </span>
            ) : (
              <span className="tone-rust">none sent. Approving would bind to nothing.</span>
            )}
          </dd>
        </div>
        <div>
          <dt>runs as</dt>
          <dd>{request.executable_identity || <span className="fainter">not stated</span>}</dd>
        </div>
        <div>
          <dt>cwd</dt>
          <dd>{request.canonical_cwd || <span className="fainter">not stated</span>}</dd>
        </div>
        <div>
          <dt>mounts</dt>
          <dd>{request.bound_mounts.length ? request.bound_mounts.join('  ') : <span className="fainter">none</span>}</dd>
        </div>
        <div>
          <dt>env</dt>
          <dd>{request.environment_names.length ? request.environment_names.join(', ') : <span className="fainter">none passed</span>}</dd>
        </div>
      </dl>

      {diff ? <DiffPreview file={diff} /> : null}

      {expired ? (
        <p className="abox-note tone-rust">Expired. The daemon will not take an answer now.</p>
      ) : (
        <>
          <ol className="abox-options" aria-label="Choices">
            {OPTIONS.map((option, i) => (
              <li key={option.key}>
                <button
                  type="button"
                  className="abox-option"
                  data-cursor={i === cursor || undefined}
                  disabled={!canDecide}
                  onMouseEnter={() => setCursor(i)}
                  onFocus={() => setCursor(i)}
                  onClick={() => choose(option)}
                >
                  <span className="abox-cursor" aria-hidden="true">
                    {i === cursor ? '❯' : ''}
                  </span>
                  <span className="abox-key">{option.key}.</span>
                  <span className={`abox-label ${option.tone === 'rust' && i === cursor ? 'tone-rust' : ''}`}>{option.label}</span>
                  {option.hint ? <kbd className="abox-hint">{option.hint}</kbd> : null}
                </button>
              </li>
            ))}
          </ol>
          {current.meaning ? <p className="abox-note tone-rust">{current.meaning}</p> : null}
          <p className="abox-legend fainter" aria-hidden="true">
            {'↑↓'} move · enter choose · y approve once · n reject · 1-5 pick
          </p>
          {askReason ? (
            <label className="abox-reason">
              <span className="fainter">reason</span>
              <input
                className="abox-input"
                value={reason}
                maxLength={500}
                autoFocus
                onChange={(e) => setReason(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    sendReason();
                  } else if (e.key === 'Escape') {
                    e.preventDefault();
                    setAskReason(false);
                    box.current?.focus();
                  }
                }}
              />
              <span className="fainter">enter sends · esc back</span>
            </label>
          ) : null}
          {!connected ? <p className="abox-note tone-rust">Not connected to the daemon, so nothing can be sent.</p> : null}
          {approval.status === 'send_failed' ? (
            <p className="abox-note tone-rust">
              Your {approval.choice === 'reject' ? 'rejection' : 'approval'} did not reach the daemon: {approval.message}. Choose again to resend.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

function DiffPreview({ file }: { file: DiffFile }) {
  const rows = file.hunks.flatMap((hunk) => [
    { kind: 'hunk' as const, text: hunk.header },
    ...hunk.lines.map((line) => ({ kind: line.kind, text: line.text })),
  ]);
  const shown = rows.slice(0, DIFF_PREVIEW_LINES);
  const more = rows.length - shown.length;
  return (
    <div className="abox-diff" aria-label={`Change to ${file.path}`}>
      <p className="abox-diff-head">
        {file.path} <span className="fainter">{file.status}</span>{' '}
        <span className="add-count">+{file.additions}</span> <span className="del-count">&minus;{file.deletions}</span>
      </p>
      <pre className="abox-diff-body">
        {shown.map((row, i) => (
          <span key={i} className={`dl dl-${row.kind}`}>
            {row.kind === 'add' ? '+' : row.kind === 'del' ? '-' : row.kind === 'hunk' ? '@' : ' '} {showControls(row.text)}
            {'\n'}
          </span>
        ))}
        {more > 0 ? <span className="fainter">{`… ${more} more lines, in Changes`}</span> : null}
      </pre>
    </div>
  );
}

function Outcome({ approval, titleId, groups }: { approval: ApprovalView; titleId: string; groups: string[] }) {
  const scope = approval.scope === 'once' || !approval.scope ? 'once' : `for this ${approval.scope}`;
  let glyph = '·';
  let tone = '';
  let text: string;
  switch (approval.status) {
    case 'sending':
      glyph = '→';
      tone = 'tone-peri';
      text = `sending your ${approval.choice === 'reject' ? 'rejection' : `approval, ${scope}`}`;
      break;
    case 'sent':
      glyph = '→';
      tone = 'tone-peri';
      text =
        approval.choice === 'reject' ? 'rejected · waiting for the daemon to move on' : `approved ${scope} · waiting for the daemon to start it`;
      break;
    case 'approved':
      glyph = '✓';
      text =
        approval.choice === 'approve'
          ? `approved ${scope} · started`
          : approval.choice === 'reject'
            ? 'rejected, and the daemon started it anyway · see the error below'
            : 'approved outside this window · started';
      if (approval.choice === 'reject') tone = 'tone-rust';
      break;
    case 'rejected':
      glyph = '✗';
      text = approval.reason ? `rejected: "${approval.reason}" · did not run` : 'rejected · did not run';
      break;
    case 'refused':
      glyph = '!';
      tone = 'tone-rust';
      text = `the daemon refused your ${approval.choice === 'reject' ? 'rejection' : 'approval'}: ${approval.message}`;
      break;
    case 'ended':
      text = 'closed · the run ended before this ran';
      break;
    default:
      text = 'no answer was sent before the request expired';
  }
  return (
    <details className="abox-done">
      <summary id={titleId}>
        <span className={`abox-done-glyph ${tone}`} aria-hidden="true">
          {glyph}
        </span>
        <span className={tone}>{text}</span>
      </summary>
      <div className="abox-done-body">
        <p>{approval.request.description}</p>
        <p className="abox-digest fainter">
          {groups.map((g, i) => (
            <span key={i}>{g}</span>
          ))}
        </p>
      </div>
    </details>
  );
}
