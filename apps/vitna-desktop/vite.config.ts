import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';

// The Vitna Code desktop frontend.
//
// It talks to nothing on the network. ADR-0005 keeps vitna-coded off loopback
// TCP on purpose, so a browser tab can never reach the daemon on its own: the
// only way in is a bridge object a desktop shell injects (src/transport/bridge.ts).
// In a plain browser that bridge is absent and the app says so.
//
// Port 5183 keeps clear of the other local dev servers on this machine
// (5173, 5175, 5177, 5179, 5180).

// The production page carries a Content Security Policy that forbids every
// origin but its own. It is added at build time only, because the dev server
// injects an inline module preamble for React refresh that a strict
// script-src would refuse. scripts/check-bundle.mjs fails the build if the
// policy goes missing.
export const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self'",
  "font-src 'self'",
  "img-src 'self' data:",
  "connect-src 'self'",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
].join('; ');

function contentSecurityPolicy(): Plugin {
  return {
    name: 'vitna-csp',
    apply: 'build',
    transformIndexHtml(html) {
      return html.replace(
        '<meta charset="UTF-8" />',
        `<meta charset="UTF-8" />\n    <meta http-equiv="Content-Security-Policy" content="${CSP}" />`,
      );
    },
  };
}

export default defineConfig({
  plugins: [react(), contentSecurityPolicy()],
  server: {
    port: 5183,
    strictPort: true,
  },
  preview: {
    port: 5184,
    strictPort: true,
  },
  build: {
    // No inline assets: a data: URI font or script would sit outside the
    // font-src and script-src 'self' the policy promises.
    assetsInlineLimit: 0,
  },
});
