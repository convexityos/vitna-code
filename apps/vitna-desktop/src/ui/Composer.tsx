import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { Notice } from '../app/Workbench';
import type { UsageUpdated } from '../protocol/types';
import type { RunSummary } from '../run/model';
import { commandsMatching, parseComposer, type ComposerIntent } from './composerInput';
import { PlusIcon, SendIcon } from './icons';
import { formatCount } from './text';
import { runStateWord } from './words';

interface ComposerProps {
  variant: 'hero' | 'dock';
  initialText: string;
  /** Why the composer cannot send, or null when it can. */
  disabledReason: string | null;
  mode: string;
  usage: UsageUpdated | null;
  run: RunSummary | null;
  runActive: boolean;
  pickFiles: (() => Promise<string[]>) | null;
  notice: Notice | null;
  onIntent(intent: ComposerIntent): Promise<boolean>;
  onPause(): void;
  onResume(): void;
  onCancel(): void;
}

const MAX_HEIGHT = 240;
const ESC_ARM_MS = 2000;

/**
 * The prompt: a box with the prompt glyph, and a status line under it. During
 * a run the box takes /steer, /pause, /resume and /cancel; esc pressed twice
 * cancels the run, once clears what was typed, which is the terminal habit
 * without letting one stray key stop a run.
 */
export function Composer(props: ComposerProps) {
  const { variant, disabledReason, usage, run, runActive, pickFiles, notice, onIntent } = props;
  const [text, setText] = useState(props.initialText);
  const [sending, setSending] = useState(false);
  const [escArmed, setEscArmed] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const hintId = useId();
  const noticeId = useId();
  const disabled = disabledReason !== null;
  const hints = commandsMatching(text);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  useEffect(() => {
    if (variant === 'dock' && !disabled) area.current?.focus();
  }, [variant, disabled]);

  useEffect(() => {
    if (!escArmed) return undefined;
    const timer = setTimeout(() => setEscArmed(false), ESC_ARM_MS);
    return () => clearTimeout(timer);
  }, [escArmed]);

  const submit = async () => {
    if (disabled || sending) return;
    const intent = parseComposer(text);
    if (intent.kind === 'empty') return;
    setSending(true);
    try {
      if (await onIntent(intent)) setText('');
    } finally {
      setSending(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void submit();
      return;
    }
    if (event.key === 'Tab' && hints.length > 0 && hints[0]) {
      event.preventDefault();
      setText(`/${hints[0].name} `);
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      if (text) {
        setText('');
        return;
      }
      if (!runActive) return;
      if (escArmed) {
        setEscArmed(false);
        props.onCancel();
      } else {
        setEscArmed(true);
      }
    }
  };

  const attach = async () => {
    const sep = text && !text.endsWith(' ') ? ' ' : '';
    if (pickFiles) {
      const paths = await pickFiles();
      if (paths.length > 0) setText(`${text}${sep}${paths.map((p) => `@${p}`).join(' ')} `);
    } else {
      setText(`${text}${sep}@`);
    }
    area.current?.focus();
  };

  const placeholder = disabled
    ? disabledReason
    : runActive
      ? '/steer to correct the run · /pause · /cancel'
      : 'Ask Vitna Code to change something. / commands, @ files.';

  const model = usage
    ? `${usage.model_sku || 'model not named'} · ${usage.provider || 'provider not named'}`
    : 'model chosen by policy';
  const word = run ? runStateWord(run.state) : null;

  return (
    <div className={`composer composer-${variant}`} data-disabled={disabled || undefined}>
      {hints.length > 0 && !disabled ? (
        <ul className="composer-hints" id={hintId} aria-label="Commands">
          {hints.map((c) => (
            <li key={c.name}>
              <button type="button" className="composer-hint" onClick={() => setText(`/${c.name} `)}>
                <span>/{c.name}</span>
                <span className="fainter">{c.argument ?? ''}</span>
                <span className="faint">{c.hint}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}

      <div className="composer-box">
        <span className="composer-glyph tone-peri" aria-hidden="true">
          {'❯'}
        </span>
        <label className="visually-hidden" htmlFor={`${hintId}-input`}>
          Prompt or command
        </label>
        <textarea
          id={`${hintId}-input`}
          ref={area}
          className="composer-input"
          rows={1}
          value={text}
          placeholder={placeholder ?? ''}
          disabled={disabled}
          aria-describedby={notice ? noticeId : undefined}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          spellCheck
        />
        <div className="composer-row">
          <button
            type="button"
            className="pill icon-pill pill-small composer-plus"
            aria-label={pickFiles ? 'Attach files' : 'Name a file with @'}
            title={pickFiles ? 'Attach files' : 'Name a file with @path'}
            disabled={disabled}
            onClick={() => void attach()}
          >
            <PlusIcon />
          </button>
          <span className="composer-meta" title="The session's mode, as the host created it">
            {props.mode || 'mode not stated'}
          </span>
          <span className="composer-meta faint" title="Reported by the daemon; this window does not choose a model">
            {model}
          </span>
          <span className="composer-spacer" />
          <button
            type="button"
            className={`pill icon-pill pill-small composer-send${text.trim() ? ' pill-solid' : ''}`}
            aria-label="Send"
            disabled={disabled || sending || !text.trim()}
            onClick={() => void submit()}
          >
            <SendIcon />
          </button>
        </div>
      </div>

      <div className="composer-status" aria-live="polite">
        {word && run ? (
          <span className={`composer-run ${word.tone === 'rust' ? 'tone-rust' : ''}`}>
            {word.word.toLowerCase()}
            {runActive ? (
              <>
                {' · '}
                {run.state === 'paused' ? (
                  <button type="button" className="composer-act" onClick={props.onResume}>
                    resume
                  </button>
                ) : (
                  <button type="button" className="composer-act" onClick={props.onPause}>
                    pause
                  </button>
                )}
                {' · '}
                <button type="button" className={`composer-act ${escArmed ? 'tone-rust' : ''}`} onClick={props.onCancel}>
                  {escArmed ? 'esc again to cancel' : 'cancel'}
                </button>
              </>
            ) : null}
          </span>
        ) : (
          <span className="fainter">enter sends · shift+enter new line · / commands · @ files</span>
        )}
        <span className="composer-spacer" />
        {usage ? (
          <span className="fainter">
            {formatCount(usage.prompt_tokens)} in · {formatCount(usage.completion_tokens)} out
            {usage.cached_tokens ? ` · ${formatCount(usage.cached_tokens)} cached` : ''} ·{' '}
            {usage.cost_known ? `$${usage.cost_usd.toFixed(4)}` : 'cost unknown'}
          </span>
        ) : word && run ? (
          <span className="fainter">esc esc cancels · / commands</span>
        ) : null}
      </div>

      {notice ? (
        <p id={noticeId} className={`composer-notice tone-${notice.tone}`} role="status">
          {notice.text}
        </p>
      ) : null}
    </div>
  );
}
