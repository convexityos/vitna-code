// The sample session's single entry point. Only ever loaded through
// `import('./sample')` behind `import.meta.env.DEV` (src/app/sampleLoader.ts),
// which is what lets a production build drop the whole directory.
export { SAMPLE_MARKER, SAMPLE_PROMPT } from './content';
export { SampleTransport } from './sampleTransport';
export { buildSampleReceipt } from './sampleReceipt';
