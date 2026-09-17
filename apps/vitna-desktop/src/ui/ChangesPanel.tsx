import { useMemo } from 'react';
import type { DiffChanged } from '../protocol/types';
import { parseUnifiedDiff, type DiffFile } from '../run/diff';
import { shortDigest, showControls } from './text';

interface ChangesPanelProps {
  diffs: DiffChanged[];
}

const STATUS_WORD: Record<DiffFile['status'], string> = {
  modified: 'modified',
  added: 'added',
  deleted: 'deleted',
  renamed: 'renamed',
};

/**
 * The agent workspace's current diff, as the daemon last reported it. It is
 * the change an approval would write, so it stays beside the timeline rather
 * than scrolling away with it.
 */
export function ChangesPanel({ diffs }: ChangesPanelProps) {
  const parsed = useMemo(() => diffs.map((d) => ({ diff: d, parsed: parseUnifiedDiff(d.diff_text) })), [diffs]);
  const totals = parsed.reduce(
    (sum, { parsed: p }) => {
      for (const f of p.files) {
        sum.add += f.additions;
        sum.del += f.deletions;
        sum.files += 1;
      }
      return sum;
    },
    { add: 0, del: 0, files: 0 },
  );

  return (
    <aside className="changes" aria-label="Changes">
      <header className="changes-head">
        <h2 className="changes-title">Changes</h2>
        <p className="mono faint">
          {totals.files} {totals.files === 1 ? 'file' : 'files'} · <span className="add-count">+{totals.add}</span>{' '}
          <span className="del-count">&minus;{totals.del}</span>
        </p>
      </header>
      <div className="changes-scroll">
        {parsed.map(({ diff, parsed: p }) => (
          <section key={diff.agent_workspace_id || 'workspace'} className="changes-workspace">
            <p className="mono fainter changes-meta">
              {diff.agent_workspace_id || 'workspace not named'} · diff sha256 {shortDigest(diff.diff_digest, 12)}
            </p>
            {diff.modified_files.length > 0 && p.files.length === 0 ? (
              <p className="faint">The daemon named {diff.modified_files.join(', ')} but sent no readable diff.</p>
            ) : null}
            {p.files.map((file) => (
              <FileDiff key={`${file.oldPath}:${file.newPath}`} file={file} />
            ))}
            {p.unplaced.length > 0 ? (
              <details className="changes-unplaced">
                <summary className="mono">Lines this window could not place ({p.unplaced.length})</summary>
                <pre>{showControls(p.unplaced.join('\n'))}</pre>
              </details>
            ) : null}
          </section>
        ))}
      </div>
    </aside>
  );
}

function FileDiff({ file }: { file: DiffFile }) {
  return (
    <article className="file-diff">
      <header className="file-head">
        <span className="file-path mono" title={file.path}>
          {file.status === 'renamed' && file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
        </span>
        <span className="mono fainter">{STATUS_WORD[file.status]}</span>
        <span className="mono">
          <span className="add-count">+{file.additions}</span> <span className="del-count">&minus;{file.deletions}</span>
        </span>
      </header>
      {file.binary ? <p className="faint file-note">Binary file. Not shown.</p> : null}
      {file.hunks.map((hunk, h) => (
        <div key={h} className="hunk">
          <p className="hunk-head mono">
            @@ &minus;{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
            {hunk.section ? <span className="fainter"> {hunk.section}</span> : null}
          </p>
          <table className="hunk-lines">
            <tbody>
              {hunk.lines.map((line, i) => (
                <tr key={i} className={`line line-${line.kind}`}>
                  <td className="ln">{line.oldNo ?? ''}</td>
                  <td className="ln">{line.newNo ?? ''}</td>
                  <td className="sign" aria-hidden="true">
                    {line.kind === 'add' ? '+' : line.kind === 'del' ? '−' : line.kind === 'note' ? '\\' : ''}
                  </td>
                  <td className="code">
                    <span className="visually-hidden">
                      {line.kind === 'add' ? 'added: ' : line.kind === 'del' ? 'removed: ' : ''}
                    </span>
                    {showControls(line.text) || ' '}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ))}
    </article>
  );
}
