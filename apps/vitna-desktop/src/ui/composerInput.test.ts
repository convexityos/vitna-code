import { describe, expect, it } from 'vitest';
import { commandsMatching, parseComposer } from './composerInput';
import { showControls } from './text';

describe('parseComposer', () => {
  it('reads plain text as a turn, and @paths as attachments', () => {
    expect(parseComposer('  fix @src/a.py and @tests/test_a.py, then @src/a.py  ')).toEqual({
      kind: 'prompt',
      prompt: 'fix @src/a.py and @tests/test_a.py, then @src/a.py',
      attachments: ['src/a.py', 'tests/test_a.py'],
    });
    expect(parseComposer('email me at dev@example.com')).toEqual({
      kind: 'prompt',
      prompt: 'email me at dev@example.com',
      attachments: [],
    });
  });

  it('reads a known slash command with its argument', () => {
    expect(parseComposer('/steer   keep the timeout at 50 ms\nand log it')).toEqual({
      kind: 'command',
      name: 'steer',
      argument: 'keep the timeout at 50 ms\nand log it',
    });
    expect(parseComposer('/PAUSE')).toEqual({ kind: 'command', name: 'pause', argument: '' });
  });

  it('names an unknown command instead of sending it as a prompt', () => {
    expect(parseComposer('/approve')).toEqual({ kind: 'unknown_command', name: 'approve' });
  });

  it('sends a prompt that starts with a slash when it is doubled', () => {
    expect(parseComposer('//etc/hosts is wrong')).toMatchObject({ kind: 'prompt', prompt: '/etc/hosts is wrong' });
    expect(parseComposer('   ')).toEqual({ kind: 'empty' });
  });

  it('lists matching commands while one is being typed', () => {
    expect(commandsMatching('/re').map((c) => c.name)).toEqual(['resume', 'receipt']);
    expect(commandsMatching('/').length).toBe(6);
    expect(commandsMatching('/steer now')).toEqual([]);
    expect(commandsMatching('hello')).toEqual([]);
  });
});

describe('showControls', () => {
  it('prints control characters as their pictures instead of dropping them', () => {
    expect(showControls('[31mred[0m\r\nok\tdone')).toBe(
      '␛[31mred␛[0m\nok\tdone␇␡�',
    );
    expect(showControls('a\rb')).toBe('a␍b');
  });
});
