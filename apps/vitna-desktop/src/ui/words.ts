/**
 * The words this window prints for protocol values. A value this table does
 * not know is printed as itself, marked, so a new state from a newer daemon
 * reads as "unknown" rather than as a blank.
 */

export type Tone = 'plain' | 'peri' | 'rust' | 'ok';

interface Word {
  word: string;
  tone: Tone;
}

const RUN_STATES: Record<string, Word> = {
  queued: { word: 'Queued', tone: 'plain' },
  running_model: { word: 'Thinking', tone: 'plain' },
  waiting_for_approval: { word: 'Waiting on you', tone: 'rust' },
  running_tool: { word: 'Running a tool', tone: 'plain' },
  waiting_for_child: { word: 'Waiting on a sub-agent', tone: 'plain' },
  waiting_for_retry: { word: 'Waiting to retry', tone: 'rust' },
  paused: { word: 'Paused', tone: 'plain' },
  needs_reconciliation: { word: 'Needs reconciliation', tone: 'rust' },
  completed: { word: 'Completed', tone: 'ok' },
  failed: { word: 'Failed', tone: 'rust' },
  cancelled: { word: 'Cancelled', tone: 'plain' },
  orphaned: { word: 'Orphaned', tone: 'rust' },
};

export function runStateWord(state: string | null | undefined): Word {
  if (!state) return { word: 'No run yet', tone: 'plain' };
  return RUN_STATES[state] ?? { word: `Unknown state "${state}"`, tone: 'rust' };
}

const EFFECTS: Record<string, string> = {
  read: 'reads',
  write: 'writes',
  execute: 'runs a program',
  network: 'uses the network',
  secret: 'uses a secret',
};

export function effectWord(effect: string): string {
  if (!effect) return 'effect not stated';
  return EFFECTS[effect] ?? `effect "${effect}"`;
}

const TOOL_TITLES: Record<string, string> = {
  read_file: 'Read',
  write_file: 'Write',
  apply_patch: 'Patch',
  list_dir: 'List',
  search_code: 'Search',
  run_command: 'Run',
  git_status: 'Git status',
  browser_verify: 'Open in a browser',
};

export function toolTitle(name: string): string {
  return TOOL_TITLES[name] ?? name;
}

/** A one-line reading of a tool's arguments, for the tools whose arguments have an obvious one. */
export function argumentSummary(name: string, argumentsJson: string): string | null {
  let args: unknown;
  try {
    args = JSON.parse(argumentsJson);
  } catch {
    return null;
  }
  if (args === null || typeof args !== 'object' || Array.isArray(args)) return null;
  const a = args as Record<string, unknown>;
  const text = (v: unknown) => (typeof v === 'string' ? v : null);
  switch (name) {
    case 'read_file':
    case 'write_file':
    case 'apply_patch':
    case 'list_dir':
      return text(a.path);
    case 'search_code': {
      const query = text(a.query);
      const path = text(a.path);
      if (!query) return null;
      return path ? `"${query}" in ${path}` : `"${query}"`;
    }
    case 'run_command': {
      if (Array.isArray(a.argv) && a.argv.every((part) => typeof part === 'string')) return a.argv.join(' ');
      return text(a.command) ?? text(a.cmd);
    }
    default:
      return null;
  }
}

const COMPLETION: Record<string, Word> = {
  completed_with_evidence: { word: 'Completed, with evidence', tone: 'ok' },
  completed_with_unknowns: { word: 'Completed, with unknowns stated', tone: 'rust' },
  blocked: { word: 'Blocked', tone: 'rust' },
  failed: { word: 'Failed', tone: 'rust' },
  cancelled: { word: 'Cancelled', tone: 'plain' },
  needs_reconciliation: { word: 'Needs reconciliation', tone: 'rust' },
};

export function completionWord(state: string): Word {
  return COMPLETION[state] ?? { word: `Unknown completion "${state}"`, tone: 'rust' };
}

const ISOLATION: Record<string, string> = {
  read_only: 'Read only',
  guarded: 'Guarded',
  strong: 'Strong',
  full_access: 'Full access',
};

export function isolationWord(label: string): string {
  return ISOLATION[label] ?? `Unknown label "${label}"`;
}

const GRADES: Record<string, string> = {
  model_reported: 'Model reported',
  broker_observed: 'Observed by the broker',
  sandbox_captured: 'Captured in the sandbox',
  independently_reproduced: 'Independently reproduced',
  remote_or_hardware_attested: 'Attested remotely or by hardware',
};

export function gradeWord(grade: string): string {
  return GRADES[grade] ?? `Unknown grade "${grade}"`;
}

export function diagnosticTone(level: string): Tone {
  return level === 'warn' || level === 'error' ? 'rust' : 'plain';
}
