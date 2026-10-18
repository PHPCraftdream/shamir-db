/**
 * Vitest setup — disable the global `fetch` so that argon2-browser's
 * Emscripten WASM loader falls back to Node's `fs.readFileSync` instead
 * of trying to HTTP-fetch the .wasm file (which fails in unit-test context).
 *
 * Node.js 18+ exposes `fetch` globally; argon2-browser detects the
 * `ENVIRONMENT_IS_NODE` flag but the Emscripten runtime still picks up
 * the global `fetch` for WASM instantiation when it is present.
 * Nulling it here forces the `readBinary` (fs) code path.
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
(globalThis as any).fetch = undefined;
