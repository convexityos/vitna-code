/**
 * Side-by-side comparison of two `vitna-run-receipt-v1` records, matching
 * `vitna receipt diff` in `apps/vitna-cli/src/commands.rs`: detects model
 * changes, isolation regressions (e.g. `guarded` -> `full_access`), modified
 * file deltas, evidence ladder changes, newly contacted egress domains, and
 * `am/1` mandate spend deltas.
 */

import { EVIDENCE_GRADES, type ModelSelection, type Receipt } from './receipt';

export interface ReceiptDiff {
  runA: string;
  runB: string;
  model: {
    changed: boolean;
    a: ModelSelection;
    b: ModelSelection;
  };
  isolation: {
    changed: boolean;
    regression: boolean;
    a: string;
    b: string;
  };
  completion: {
    changed: boolean;
    a: string;
    b: string;
  };
  files: {
    onlyInA: string[];
    onlyInB: string[];
    postimageChanged: string[];
    unchanged: string[];
  };
  evidence: {
    countA: number;
    countB: number;
    highestGradeA: string;
    highestGradeB: string;
  };
  egress: {
    countA: number;
    countB: number;
    newDomains: string[];
  };
  mandate: {
    rootA: string | null;
    rootB: string | null;
    spentA: string | null;
    spentB: string | null;
  };
}

function isolationStrengthRank(label: string): number | null {
  switch (label) {
    case 'full_access':
      return 0;
    case 'guarded':
      return 1;
    case 'read_only':
      return 2;
    case 'strong':
      return 3;
    default:
      return null;
  }
}

export function highestEvidenceGrade(receipt: Receipt): string {
  let bestIdx = -1;
  for (const item of receipt.evidence_items) {
    const idx = (EVIDENCE_GRADES as readonly string[]).indexOf(item.grade);
    if (idx > bestIdx) bestIdx = idx;
  }
  return bestIdx >= 0 ? (EVIDENCE_GRADES[bestIdx] ?? 'none') : 'none';
}

export function diffReceipts(a: Receipt, b: Receipt): ReceiptDiff {
  const filesA = new Map<string, string>();
  for (const f of a.changeset.files_modified) {
    filesA.set(f.path, f.postimage_hash);
  }
  const filesB = new Map<string, string>();
  for (const f of b.changeset.files_modified) {
    filesB.set(f.path, f.postimage_hash);
  }

  const onlyInA: string[] = [];
  const onlyInB: string[] = [];
  const postimageChanged: string[] = [];
  const unchanged: string[] = [];

  for (const [path, hashA] of filesA) {
    const hashB = filesB.get(path);
    if (hashB === undefined) {
      onlyInA.push(path);
    } else if (hashB !== hashA) {
      postimageChanged.push(path);
    } else {
      unchanged.push(path);
    }
  }
  for (const path of filesB.keys()) {
    if (!filesA.has(path)) {
      onlyInB.push(path);
    }
  }

  onlyInA.sort();
  onlyInB.sort();
  postimageChanged.sort();
  unchanged.sort();

  const modelChanged =
    a.model_selection.provider !== b.model_selection.provider ||
    a.model_selection.model_sku !== b.model_selection.model_sku ||
    a.model_selection.policy_digest !== b.model_selection.policy_digest;

  const isolationChanged = a.isolation_label !== b.isolation_label;
  const rankA = isolationStrengthRank(a.isolation_label);
  const rankB = isolationStrengthRank(b.isolation_label);
  const isolationRegression = rankA !== null && rankB !== null && rankB < rankA;

  const domainsA = new Set(a.egress_calls.map((e) => e.target_domain));
  const newDomainsSet = new Set<string>();
  for (const call of b.egress_calls) {
    if (!domainsA.has(call.target_domain)) {
      newDomainsSet.add(call.target_domain);
    }
  }
  const newDomains = Array.from(newDomainsSet).sort();

  const spentA = a.mandate_provenance
    ? `${a.mandate_provenance.total_spent} ${a.mandate_provenance.currency}`
    : null;
  const spentB = b.mandate_provenance
    ? `${b.mandate_provenance.total_spent} ${b.mandate_provenance.currency}`
    : null;

  return {
    runA: a.run_id,
    runB: b.run_id,
    model: {
      changed: modelChanged,
      a: a.model_selection,
      b: b.model_selection,
    },
    isolation: {
      changed: isolationChanged,
      regression: isolationRegression,
      a: a.isolation_label,
      b: b.isolation_label,
    },
    completion: {
      changed: a.completion_state !== b.completion_state,
      a: a.completion_state,
      b: b.completion_state,
    },
    files: {
      onlyInA,
      onlyInB,
      postimageChanged,
      unchanged,
    },
    evidence: {
      countA: a.evidence_items.length,
      countB: b.evidence_items.length,
      highestGradeA: highestEvidenceGrade(a),
      highestGradeB: highestEvidenceGrade(b),
    },
    egress: {
      countA: a.egress_calls.length,
      countB: b.egress_calls.length,
      newDomains,
    },
    mandate: {
      rootA: a.mandate_provenance?.root_jti ?? null,
      rootB: b.mandate_provenance?.root_jti ?? null,
      spentA,
      spentB,
    },
  };
}
