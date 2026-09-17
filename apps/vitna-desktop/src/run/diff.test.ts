import { describe, expect, it } from 'vitest';
import { parseUnifiedDiff } from './diff';

describe('parseUnifiedDiff', () => {
  it('numbers the lines of a git diff hunk on both sides', () => {
    const { files, unplaced } = parseUnifiedDiff(
      [
        'diff --git a/src/a.rs b/src/a.rs',
        'index 111..222 100644',
        '--- a/src/a.rs',
        '+++ b/src/a.rs',
        '@@ -10,4 +10,5 @@ fn main() {',
        ' keep one',
        '-old',
        '+new one',
        '+new two',
        ' keep two',
        ' keep three',
        '',
      ].join('\n'),
    );
    expect(unplaced).toEqual([]);
    expect(files).toHaveLength(1);
    const [file] = files;
    expect(file).toMatchObject({ path: 'src/a.rs', status: 'modified', additions: 2, deletions: 1, binary: false });
    const hunk = file?.hunks[0];
    expect(hunk?.section).toBe('fn main() {');
    expect(hunk?.lines.map((l) => [l.kind, l.oldNo, l.newNo])).toEqual([
      ['context', 10, 10],
      ['del', 11, null],
      ['add', null, 11],
      ['add', null, 12],
      ['context', 12, 13],
      ['context', 13, 14],
    ]);
  });

  it('reads a removed line that looks like a file header as a removed line', () => {
    const { files } = parseUnifiedDiff(
      ['--- a/notes.md', '+++ b/notes.md', '@@ -1,2 +1,1 @@', '--- a/looks/like/a/header', ' tail', ''].join('\n'),
    );
    expect(files).toHaveLength(1);
    expect(files[0]?.hunks[0]?.lines.map((l) => [l.kind, l.text])).toEqual([
      ['del', '-- a/looks/like/a/header'],
      ['context', 'tail'],
    ]);
  });

  it('splits a plain diff with several files', () => {
    const { files } = parseUnifiedDiff(
      [
        '--- a/one.txt',
        '+++ b/one.txt',
        '@@ -1 +1 @@',
        '-a',
        '+b',
        '--- a/two.txt',
        '+++ b/two.txt',
        '@@ -1 +1,2 @@',
        ' x',
        '+y',
      ].join('\n'),
    );
    expect(files.map((f) => [f.path, f.additions, f.deletions])).toEqual([
      ['one.txt', 1, 1],
      ['two.txt', 1, 0],
    ]);
  });

  it('names added, deleted, renamed and binary files', () => {
    const { files } = parseUnifiedDiff(
      [
        'diff --git a/new.txt b/new.txt',
        'new file mode 100644',
        '--- /dev/null',
        '+++ b/new.txt',
        '@@ -0,0 +1 @@',
        '+hello',
        'diff --git a/gone.txt b/gone.txt',
        'deleted file mode 100644',
        '--- a/gone.txt',
        '+++ /dev/null',
        '@@ -1 +0,0 @@',
        '-bye',
        'diff --git a/old/name.rs b/new/name.rs',
        'similarity index 100%',
        'rename from old/name.rs',
        'rename to new/name.rs',
        'diff --git a/logo.png b/logo.png',
        'Binary files a/logo.png and b/logo.png differ',
      ].join('\n'),
    );
    expect(files.map((f) => [f.path, f.status, f.binary])).toEqual([
      ['new.txt', 'added', false],
      ['gone.txt', 'deleted', false],
      ['new/name.rs', 'renamed', false],
      ['logo.png', 'modified', true],
    ]);
    expect(files[2]?.oldPath).toBe('old/name.rs');
  });

  it('keeps the no-newline marker and anything it cannot place', () => {
    const { files, unplaced } = parseUnifiedDiff(
      [
        'a preamble line',
        '--- a/x',
        '+++ b/x',
        '@@ -1 +1 @@',
        '-a',
        '\\ No newline at end of file',
        '+a',
        '\\ No newline at end of file',
      ].join('\n'),
    );
    expect(unplaced).toEqual(['a preamble line']);
    expect(files[0]?.hunks[0]?.lines.map((l) => l.kind)).toEqual(['del', 'note', 'add', 'note']);
    expect(files[0]?.hunks[0]?.lines[1]?.text).toBe('No newline at end of file');
  });

  it('keeps text that is not a diff at all', () => {
    expect(parseUnifiedDiff('just words\nmore words\n')).toEqual({
      files: [],
      unplaced: ['just words', 'more words'],
    });
  });
});
