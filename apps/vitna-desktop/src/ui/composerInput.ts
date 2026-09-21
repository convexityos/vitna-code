/**
 * What the composer's text means. Plain text is a turn; `/name` is one of the
 * commands below, each of which maps to a protocol command or to a place in
 * this window; `@path` names a file to attach, and stays in the prompt too so
 * the model reads the same words the operator wrote.
 */

export const COMMANDS = [
  { name: 'steer', hint: 'Correct the run that is going now', argument: 'what to change' },
  { name: 'pause', hint: 'Pause the run', argument: null },
  { name: 'resume', hint: 'Resume a paused run', argument: null },
  { name: 'cancel', hint: 'Cancel the run', argument: null },
  { name: 'receipt', hint: 'Open the receipt reader', argument: null },
  { name: 'providers', hint: 'See the model providers the host offers', argument: null },
] as const;

export type CommandName = (typeof COMMANDS)[number]['name'];

export type ComposerIntent =
  | { kind: 'empty' }
  | { kind: 'prompt'; prompt: string; attachments: string[] }
  | { kind: 'command'; name: CommandName; argument: string }
  | { kind: 'unknown_command'; name: string };

const ATTACHMENT = /(?:^|\s)@([^\s@]+)/g;

export function parseComposer(text: string): ComposerIntent {
  const trimmed = text.trim();
  if (!trimmed) return { kind: 'empty' };

  if (trimmed.startsWith('/') && !trimmed.startsWith('//')) {
    const match = /^\/(\S+)(?:\s+([\s\S]*))?$/.exec(trimmed);
    const name = (match?.[1] ?? '').toLowerCase();
    const command = COMMANDS.find((c) => c.name === name);
    if (!command) return { kind: 'unknown_command', name };
    return { kind: 'command', name: command.name, argument: (match?.[2] ?? '').trim() };
  }

  // "//" sends a prompt that starts with a slash.
  const prompt = trimmed.startsWith('//') ? trimmed.slice(1) : trimmed;
  const attachments: string[] = [];
  for (const match of prompt.matchAll(ATTACHMENT)) {
    const path = (match[1] ?? '').replace(/[),.;:!?]+$/, '');
    if (path && !attachments.includes(path)) attachments.push(path);
  }
  return { kind: 'prompt', prompt, attachments };
}

/** Commands whose names start with what has been typed after the slash, for the hint list. */
export function commandsMatching(text: string): (typeof COMMANDS)[number][] {
  const match = /^\/(\S*)$/.exec(text.trim());
  if (!match) return [];
  const typed = (match[1] ?? '').toLowerCase();
  return COMMANDS.filter((c) => c.name.startsWith(typed));
}
