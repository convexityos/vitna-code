import { useCallback, useEffect, useRef, useState } from 'react';
import type { Transport } from '../transport/types';
import { Workbench } from './Workbench';
import { bridgeTransport, loadSample } from './transportChoice';

let sampleCount = 0;

interface Choice {
  key: string;
  transport: Transport | null;
  initialPrompt: string;
}

function initialChoice(): Choice {
  const bridge = bridgeTransport();
  return { key: bridge ? 'bridge' : 'none', transport: bridge, initialPrompt: '' };
}

/**
 * Picks what this window talks to: the host's bridge when there is one, and
 * otherwise nothing, until a developer opens the sample. The workbench is
 * keyed by that choice so a new connection always starts from an empty view.
 */
export function App() {
  const [choice, setChoice] = useState<Choice>(initialChoice);
  const [sampleError, setSampleError] = useState('');
  const opening = useRef(false);

  const openSample = useCallback(async () => {
    if (!loadSample || opening.current) return;
    opening.current = true;
    try {
      const sample = await loadSample();
      sampleCount += 1;
      setChoice({
        key: `sample-${sampleCount}`,
        transport: new sample.SampleTransport(),
        initialPrompt: sample.SAMPLE_PROMPT,
      });
    } catch (error) {
      setSampleError(error instanceof Error ? error.message : String(error));
    } finally {
      opening.current = false;
    }
  }, []);

  // In development, /?sample opens straight into the sample session.
  useEffect(() => {
    if (!loadSample || choice.transport) return;
    if (new URLSearchParams(window.location.search).has('sample')) void openSample();
  }, [choice.transport, openSample]);

  return (
    <Workbench
      key={choice.key}
      transport={choice.transport}
      initialPrompt={choice.initialPrompt}
      onOpenSample={loadSample && !choice.transport ? openSample : null}
      sampleError={sampleError}
    />
  );
}
