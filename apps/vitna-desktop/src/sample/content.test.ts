import { describe, expect, it } from 'vitest';
import { parseUnifiedDiff, type DiffFile } from '../run/diff';
import { SERVICE_AFTER, SERVICE_BEFORE, SERVICE_DIFF, SERVICE_PATH } from './content';

/** Applies a parsed diff strictly: every context and removed line must match the old text. */
function apply(before: string, file: DiffFile): string {
  const old = before.split('\n');
  old.pop();
  const out: string[] = [];
  let cursor = 1;
  for (const hunk of file.hunks) {
    while (cursor < hunk.oldStart) out.push(old[cursor++ - 1] as string);
    for (const line of hunk.lines) {
      if (line.kind === 'add') {
        out.push(line.text);
        continue;
      }
      if (line.kind === 'note') continue;
      expect(old[cursor - 1], `line ${cursor}`).toBe(line.text);
      if (line.kind === 'context') out.push(line.text);
      cursor += 1;
    }
  }
  while (cursor <= old.length) out.push(old[cursor++ - 1] as string);
  return `${out.join('\n')}\n`;
}

describe('the sample change', () => {
  it('is a diff that turns the before file into the after file', () => {
    const { files, unplaced } = parseUnifiedDiff(SERVICE_DIFF);
    expect(unplaced).toEqual([]);
    expect(files).toHaveLength(1);
    const file = files[0] as DiffFile;
    expect(file.path).toBe(SERVICE_PATH);
    expect(apply(SERVICE_BEFORE, file)).toBe(SERVICE_AFTER);
  });

  it('declares hunk sizes that match its lines, and the counts its description states', () => {
    const file = parseUnifiedDiff(SERVICE_DIFF).files[0] as DiffFile;
    for (const hunk of file.hunks) {
      expect(hunk.lines.filter((l) => l.kind !== 'add' && l.kind !== 'note')).toHaveLength(hunk.oldLines);
      expect(hunk.lines.filter((l) => l.kind !== 'del' && l.kind !== 'note')).toHaveLength(hunk.newLines);
    }
    // sampleTransport describes this action as 13 added lines and 1 removed line.
    expect([file.additions, file.deletions]).toEqual([13, 1]);
  });
});
