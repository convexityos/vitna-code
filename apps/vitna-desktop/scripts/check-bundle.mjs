// Fails the build when dist/ carries something a production window must not:
//
// 1. the Content Security Policy that keeps the page to its own origin;
// 2. any part of the DEV-only sample session, which is a scripted run and
//    would read as a real one if it ever shipped;
// 3. a reference to a third-party host, since this window promises that
//    nothing it shows leaves the machine (no web fonts, no CDNs);
// 4. an inline script, which the policy would refuse at runtime anyway.
//
// The sample marker is read out of src/sample/content.ts rather than copied,
// so renaming it cannot quietly turn this check into one that always passes.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const dist = join(root, 'dist');

const content = readFileSync(join(root, 'src/sample/content.ts'), 'utf8');
const marker = /export const SAMPLE_MARKER = '([^']+)'/.exec(content)?.[1];
if (!marker) fail('could not read SAMPLE_MARKER from src/sample/content.ts');

const sampleTells = [marker, 'SampleTransport', 'buildSampleReceipt', 'test_fails_open_when_redis_times_out'];
const thirdParty = /\b(?:fonts\.googleapis\.com|fonts\.gstatic\.com|cdn\.jsdelivr\.net|unpkg\.com|cdnjs\.cloudflare\.com)\b/;

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

function fail(message) {
  console.error(`check-bundle: ${message}`);
  process.exit(1);
}

const files = walk(dist);
const problems = [];

const html = readFileSync(join(dist, 'index.html'), 'utf8');
if (!/<meta http-equiv="Content-Security-Policy" content="default-src 'self';/.test(html)) {
  problems.push("index.html has no Content-Security-Policy starting with default-src 'self'");
}
if (/<script(?![^>]*\bsrc=)[^>]*>/.test(html)) {
  problems.push('index.html carries an inline script');
}

for (const file of files) {
  if (!/\.(?:html|js|css|json|map|txt|svg)$/.test(file)) continue;
  const text = readFileSync(file, 'utf8');
  const name = relative(dist, file);
  for (const tell of sampleTells) {
    if (text.includes(tell)) problems.push(`${name} contains "${tell}", which belongs to the DEV-only sample`);
  }
  const host = thirdParty.exec(text);
  if (host) problems.push(`${name} references ${host[0]}`);
}

if (problems.length) fail(`\n  ${problems.join('\n  ')}`);
console.log(`check-bundle: ${files.length} files; policy present; no sample; no third-party hosts`);
