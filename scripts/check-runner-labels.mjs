// Validate every runner label named in .github/workflows against the set of
// GitHub-hosted labels this repository is allowed to use.
//
// Why this exists: a runner label that matches no runner does not fail. The job
// is queued instead, and GitHub cancels it 24 hours later, which makes the whole
// run conclude "cancelled" without a single failing step. Between 2026-09-16 and
// 2026-09-20 every "Vitna Code CI" run concluded that way, because the Windows
// ARM64 leg asked for "windows-11-arm64" and the label GitHub publishes is
// "windows-11-arm". The other six jobs passed the entire time, so the red was
// indistinguishable from a real failure and five pull requests sat unmerged.
//
// What this proves: that every label named here is spelled like one GitHub
// offers. It does not prove a runner is available to this repository, and it
// cannot: only a real run shows that.

import { readFileSync, readdirSync } from 'node:fs';
import { join, sep } from 'node:path';

const WORKFLOW_DIR = join('.github', 'workflows');

// GitHub-hosted runner labels, checked against GitHub's published list on
// 2026-09-20. This is deliberately fail-closed: a label GitHub adds later is
// rejected until it is added here. That costs one loud red check and a one-line
// edit, where the alternative costs 24 hours of silence per run.
const ALLOWED = new Set([
  'ubuntu-latest',
  'ubuntu-26.04',
  'ubuntu-24.04',
  'ubuntu-22.04',
  'ubuntu-26.04-arm',
  'ubuntu-24.04-arm',
  'ubuntu-22.04-arm',
  'windows-latest',
  'windows-2025',
  'windows-2022',
  'windows-11-arm',
  'windows-11-vs2026-arm',
  'macos-latest',
  'macos-26',
  'macos-15',
  'macos-15-intel',
  'macos-14',
  'macos-13',
]);

const KEY_RUNS_ON = 'runs-on:';
const EXPR_OPEN = '${{';
const EXPR_CLOSE = '}}';
const LIST_ITEM = /^-\s*/;
const LINE_BREAK = /\r?\n/;

function stripQuotes(value) {
  const trimmed = value.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    const last = trimmed[trimmed.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return trimmed.slice(1, -1).trim();
    }
  }
  return trimmed;
}

// "${{ matrix.target.runner }}" resolves to the matrix key name "runner".
function matrixKeyOf(value) {
  const open = value.indexOf(EXPR_OPEN);
  const close = value.indexOf(EXPR_CLOSE);
  if (open === -1 || close === -1 || close < open) return null;
  const inner = value.slice(open + EXPR_OPEN.length, close).trim();
  if (!inner.startsWith('matrix.')) return null;
  const parts = inner.split('.');
  const key = parts[parts.length - 1].trim();
  return key.length > 0 ? key : null;
}

// A runs-on may be a list: "runs-on: [self-hosted, linux]".
function expandList(value) {
  if (!value.startsWith('[') || !value.endsWith(']')) return [value];
  return value
    .slice(1, -1)
    .split(',')
    .map((part) => stripQuotes(part))
    .filter((part) => part.length > 0);
}

// Every value assigned to `key` anywhere in the file, with the line it sits on,
// for resolving a matrix reference without taking on a YAML parser dependency.
// The line matters: the offending label lives in the matrix block, which is
// often dozens of lines from the runs-on that refers to it.
function valuesForKey(lines, key) {
  const wanted = key + ':';
  const found = [];
  for (const [index, line] of lines.entries()) {
    const trimmed = line.trim().replace(LIST_ITEM, '');
    if (!trimmed.startsWith(wanted)) continue;
    const value = stripQuotes(trimmed.slice(wanted.length));
    if (value.length > 0) found.push({ value, line: index + 1 });
  }
  return found;
}

function labelsIn(rawFile, text) {
  const file = rawFile.split(sep).join('/');
  const lines = text.split(LINE_BREAK);
  const labels = [];
  for (const [index, line] of lines.entries()) {
    const trimmed = line.trim().replace(LIST_ITEM, '');
    if (!trimmed.startsWith(KEY_RUNS_ON)) continue;
    const raw = stripQuotes(trimmed.slice(KEY_RUNS_ON.length));
    if (raw.length === 0) continue;
    const key = matrixKeyOf(raw);
    if (key) {
      const resolved = valuesForKey(lines, key);
      if (resolved.length === 0) {
        labels.push({
          where: file + ':' + (index + 1),
          label: raw,
          unresolved: true,
        });
        continue;
      }
      for (const entry of resolved) {
        for (const item of expandList(entry.value)) {
          labels.push({
            where:
              file + ':' + entry.line + ' (matrix.' + key +
              ', reached from runs-on at line ' + (index + 1) + ')',
            label: item,
          });
        }
      }
      continue;
    }
    for (const item of expandList(raw)) {
      labels.push({ where: file + ':' + (index + 1), label: item });
    }
  }
  return labels;
}

const files = readdirSync(WORKFLOW_DIR).filter(
  (name) => name.endsWith('.yml') || name.endsWith('.yaml'),
);

const problems = [];
let checked = 0;

for (const name of files) {
  const path = join(WORKFLOW_DIR, name);
  for (const entry of labelsIn(path, readFileSync(path, 'utf8'))) {
    checked += 1;
    if (entry.unresolved) {
      problems.push(entry.where + ': could not resolve ' + entry.label);
      continue;
    }
    if (!ALLOWED.has(entry.label)) {
      problems.push(entry.where + ': unknown runner label "' + entry.label + '"');
    }
  }
}

if (problems.length > 0) {
  console.error('Unusable runner labels found:');
  for (const problem of problems) console.error('  ' + problem);
  console.error('');
  console.error('A label that matches no runner does not fail the job. It queues,');
  console.error('and GitHub cancels it 24 hours later, so the run concludes');
  console.error('"cancelled" with every step still green.');
  console.error('');
  console.error('If GitHub has published a new label, add it to ALLOWED in');
  console.error('scripts/check-runner-labels.mjs. Otherwise correct the spelling.');
  process.exit(1);
}

console.log(
  'Runner labels checked: ' + checked + ' across ' + files.length +
  ' workflow file(s). All known.',
);
