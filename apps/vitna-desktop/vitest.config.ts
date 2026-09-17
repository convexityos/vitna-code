import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// Logic tests run in node, where WebCrypto carries Ed25519. Component tests
// opt into a DOM per file with a `@vitest-environment happy-dom` docblock.
// happy-dom rather than jsdom on purpose: jsdom took 11 seconds to import on
// the Windows ARM64 machine this was built on and its vitest worker never
// came up, while happy-dom starts in well under a second.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    css: false,
  },
});
