/**
 * Unified diff parsing for DiffChanged.diff_text.
 *
 * Accepts git's extended headers and plain `---`/`+++` diffs. Anything the
 * parser cannot place is kept in `unplaced` and shown as it came, so a diff
 * this window does not understand is never rendered as a shorter, cleaner
 * diff than the one the daemon sent.
 */

export type DiffLineKind = 'context' | 'add' | 'del' | 'note';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

export interface DiffHunk {
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  section: string;
  lines: DiffLine[];
}

export type DiffFileStatus = 'modified' | 'added' | 'deleted' | 'renamed';

export interface DiffFile {
  oldPath: string | null;
  newPath: string | null;
  path: string;
  status: DiffFileStatus;
  binary: boolean;
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
}

export interface ParsedDiff {
  files: DiffFile[];
  unplaced: string[];
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;

function stripPrefix(path: string): string | null {
  const bare = path.replace(/\t.*$/, '').trim();
  if (bare === '/dev/null') return null;
  return bare.replace(/^[ab]\//, '');
}

function newFile(): DiffFile {
  return {
    oldPath: null,
    newPath: null,
    path: '',
    status: 'modified',
    binary: false,
    hunks: [],
    additions: 0,
    deletions: 0,
  };
}

function finish(file: DiffFile): DiffFile {
  const path = file.newPath ?? file.oldPath ?? '(unnamed file)';
  let status = file.status;
  if (status === 'modified') {
    if (file.oldPath === null && file.newPath !== null) status = 'added';
    else if (file.newPath === null && file.oldPath !== null) status = 'deleted';
    else if (file.oldPath !== null && file.newPath !== null && file.oldPath !== file.newPath) status = 'renamed';
  }
  return { ...file, path, status };
}

export function parseUnifiedDiff(text: string): ParsedDiff {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();

  const files: DiffFile[] = [];
  const unplaced: string[] = [];
  let file: DiffFile | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  let oldLeft = 0;
  let newLeft = 0;

  const closeFile = () => {
    if (file) files.push(finish(file));
    file = null;
    hunk = null;
  };

  for (const line of lines) {
    // Inside a hunk, body lines are consumed by count before any header check,
    // because a removed line can itself begin with "--- ".
    if (file && hunk && (oldLeft > 0 || newLeft > 0)) {
      const mark = line[0];
      const body = line.slice(1);
      if (mark === ' ' || line === '') {
        hunk.lines.push({ kind: 'context', text: body, oldNo, newNo });
        oldNo += 1;
        newNo += 1;
        oldLeft -= 1;
        newLeft -= 1;
        continue;
      }
      if (mark === '-') {
        hunk.lines.push({ kind: 'del', text: body, oldNo, newNo: null });
        (file as DiffFile).deletions += 1;
        oldNo += 1;
        oldLeft -= 1;
        continue;
      }
      if (mark === '+') {
        hunk.lines.push({ kind: 'add', text: body, oldNo: null, newNo });
        (file as DiffFile).additions += 1;
        newNo += 1;
        newLeft -= 1;
        continue;
      }
      if (mark === '\\') {
        hunk.lines.push({ kind: 'note', text: line.slice(2), oldNo: null, newNo: null });
        continue;
      }
      // A line that is none of the above ends the hunk early; fall through.
      oldLeft = 0;
      newLeft = 0;
    }

    if (line.startsWith('\\') && hunk) {
      hunk.lines.push({ kind: 'note', text: line.slice(2), oldNo: null, newNo: null });
      continue;
    }

    if (line.startsWith('diff --git ')) {
      closeFile();
      file = newFile();
      const m = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
      if (m) {
        file.oldPath = m[1] ?? null;
        file.newPath = m[2] ?? null;
      }
      continue;
    }

    if (line.startsWith('--- ') && (!file || file.hunks.length > 0 || hunk)) {
      // A plain diff with no git header, or the next file of a plain diff.
      if (!file || file.hunks.length > 0) {
        closeFile();
        file = newFile();
      }
    }

    if (file) {
      const current: DiffFile = file;
      if (line.startsWith('--- ')) {
        current.oldPath = stripPrefix(line.slice(4));
        continue;
      }
      if (line.startsWith('+++ ')) {
        current.newPath = stripPrefix(line.slice(4));
        continue;
      }
      if (line.startsWith('new file mode')) {
        current.status = 'added';
        current.oldPath = null;
        continue;
      }
      if (line.startsWith('deleted file mode')) {
        current.status = 'deleted';
        current.newPath = null;
        continue;
      }
      if (line.startsWith('rename from ')) {
        current.status = 'renamed';
        current.oldPath = line.slice('rename from '.length);
        continue;
      }
      if (line.startsWith('rename to ')) {
        current.status = 'renamed';
        current.newPath = line.slice('rename to '.length);
        continue;
      }
      if (line.startsWith('Binary files ') || line === 'GIT binary patch') {
        current.binary = true;
        continue;
      }
      if (/^(index |old mode |new mode |similarity index |dissimilarity index |copy from |copy to )/.test(line)) {
        continue;
      }
      const m = HUNK.exec(line);
      if (m) {
        hunk = {
          header: line,
          oldStart: Number(m[1]),
          oldLines: m[2] === undefined ? 1 : Number(m[2]),
          newStart: Number(m[3]),
          newLines: m[4] === undefined ? 1 : Number(m[4]),
          section: m[5] ?? '',
          lines: [],
        };
        current.hunks.push(hunk);
        oldNo = hunk.oldStart;
        newNo = hunk.newStart;
        oldLeft = hunk.oldLines;
        newLeft = hunk.newLines;
        continue;
      }
    }

    unplaced.push(line);
  }
  closeFile();
  return { files, unplaced };
}
