import { BridgeTransport, findBridge } from '../transport/bridge';
import type { Transport } from '../transport/types';

export function bridgeTransport(): Transport | null {
  const bridge = findBridge();
  return bridge ? new BridgeTransport(bridge) : null;
}

export type SampleModule = typeof import('../sample');

/**
 * The sample session's loader, or null in a production build.
 *
 * Written as a ternary on import.meta.env.DEV on purpose: the build replaces
 * the flag with `false`, the dead branch goes, and the dynamic import goes
 * with it, so no chunk of src/sample is emitted. A guard inside the function
 * body would leave that to the minifier's reachability analysis.
 * scripts/check-bundle.mjs fails the build if the sample reaches dist anyway.
 */
export const loadSample: (() => Promise<SampleModule>) | null = import.meta.env.DEV
  ? () => import('../sample')
  : null;
