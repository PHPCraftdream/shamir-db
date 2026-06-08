import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // Disable the global `fetch` so argon2-browser uses Node.js fs to load
    // the WASM file instead of fetching via HTTP (Node 18+ exposes fetch
    // globally, which confuses the argon2-browser env detection).
    setupFiles: ['./vitest.setup.ts'],
    testTimeout: 90_000,
  },
});
