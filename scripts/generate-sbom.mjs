#!/usr/bin/env node
// Generate a complete SPDX 2.3 SBOM from the resolved Cargo dependency graph.
//
// Usage:
//   node scripts/generate-sbom.mjs [--out path/to/sbom.spdx.json] [--check-licenses]
//
// Unlike hand-authored SBOMs that list only a sample of dependencies, this
// script reads every resolved package from `cargo metadata --format-version 1`.

import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';

const COPYLEFT_PATTERNS = [/\bAGPL\b/i, /\bGPL\b/i, /\bLGPL\b/i, /\bSSPL\b/i, /\bBUSL\b/i];

function parseArgs(argv) {
  let out = null;
  let checkLicenses = false;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--out') {
      out = argv[i + 1] ?? null;
      i += 1;
    } else if (arg === '--check-licenses') {
      checkLicenses = true;
    } else if (arg === '--help' || arg === '-h') {
      console.log('Usage: node scripts/generate-sbom.mjs [--out <path>] [--check-licenses]');
      process.exit(0);
    }
  }
  return { out, checkLicenses };
}

function spdxId(name, version) {
  const safe = `${name}-${version}`.replace(/[^A-Za-z0-9.-]/g, '-');
  return `SPDXRef-Package-${safe}`;
}

function isCopyleft(license) {
  if (!license || license === 'NOASSERTION') return false;
  // Dual-licensed expressions with OR (for example "MIT OR Apache-2.0") are
  // acceptable when at least one branch is permissive.
  const branches = license.split(/\s+OR\s+/i);
  return branches.every((branch) => COPYLEFT_PATTERNS.some((re) => re.test(branch)));
}

const { out, checkLicenses } = parseArgs(process.argv.slice(2));
const raw = execFileSync('cargo', ['metadata', '--format-version', '1'], {
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
});
const metadata = JSON.parse(raw);

const workspaceMembers = new Set(metadata.workspace_members ?? []);
const packages = [...(metadata.packages ?? [])].sort((a, b) =>
  a.name === b.name ? a.version.localeCompare(b.version) : a.name.localeCompare(b.name),
);

const spdxPackages = [];
const relationships = [];
const copyleftFindings = [];

for (const pkg of packages) {
  const id = spdxId(pkg.name, pkg.version);
  const license = pkg.license || 'NOASSERTION';
  const isWorkspace = workspaceMembers.has(pkg.id);

  if (!isWorkspace && isCopyleft(license)) {
    copyleftFindings.push({ name: pkg.name, version: pkg.version, license });
  }

  spdxPackages.push({
    SPDXID: id,
    name: pkg.name,
    versionInfo: pkg.version,
    downloadLocation: pkg.source || 'NOASSERTION',
    filesAnalyzed: false,
    licenseConcluded: license,
    licenseDeclared: license,
    copyrightText: 'NOASSERTION',
    supplier: isWorkspace ? 'Organization: Vitna Engineering' : 'NOASSERTION',
  });

  if (isWorkspace) {
    relationships.push({
      spdxElementId: 'SPDXRef-DOCUMENT',
      relationshipType: 'DESCRIBES',
      relatedSpdxElement: id,
    });
  }
}

const created = new Date().toISOString();
const sbom = {
  spdxVersion: 'SPDX-2.3',
  dataLicense: 'CC0-1.0',
  SPDXID: 'SPDXRef-DOCUMENT',
  name: 'vitna-code-sbom',
  documentNamespace: `https://vitna.ai/spdx/vitna-code-${created}`,
  creationInfo: {
    created,
    creators: ['Tool: vitna-code/scripts/generate-sbom.mjs'],
    comment: `Generated from cargo metadata (${packages.length} total packages, ${workspaceMembers.size} workspace members).`,
  },
  packages: spdxPackages,
  relationships,
};

const json = `${JSON.stringify(sbom, null, 2)}\n`;
if (out) {
  writeFileSync(out, json, 'utf8');
  console.log(
    `Wrote SPDX 2.3 SBOM to ${out}: ${packages.length} packages (${workspaceMembers.size} workspace, ${packages.length - workspaceMembers.size} external).`,
  );
} else {
  process.stdout.write(json);
}

if (checkLicenses) {
  if (copyleftFindings.length > 0) {
    console.error('Copyleft-only dependencies detected:');
    for (const finding of copyleftFindings) {
      console.error(`  - ${finding.name} ${finding.version}: ${finding.license}`);
    }
    process.exit(1);
  }
  console.error(
    `License check passed: 0 copyleft-only external packages across ${packages.length - workspaceMembers.size} external crates.`,
  );
}
