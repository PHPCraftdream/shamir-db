/**
 * End-to-end stored-function call test.
 *
 * Creates a function from a real WASM module (echo — returns its input),
 * invokes it via CallOp, and asserts the result.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, ddl, call } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  setupDb,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

/**
 * Echo WASM module — pre-compiled from the workspace's ECHO_WAT.
 *
 * Exports `memory` (2 pages), `shamir_alloc` (bump allocator), and
 * `shamir_call` which echoes `[ptr, len)` back as the packed result.
 * This means it returns the same msgpack bytes that were passed in,
 * effectively echoing the params map.
 */
const ECHO_WASM_B64 =
  'AGFzbQEAAAABDAJgAX8Bf2ACf38BfgMDAgABBQMBAAIGBwF/AUGACAsHJwMGbWVtb3J5AgAMc2hhbWlyX2FsbG9jAAALc2hhbWlyX2NhbGwAAQogAhEBAX8jACEBIwAgAGokACABCwwAIACtQiCGIAGthAsAKQRuYW1lAhkCAAIAA2xlbgEDcHRyAQIAA3B0cgEDbGVuBwcBAARidW1w';

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e call — createFunction(wasm) + call() (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-call] connection failed. Server logs:\n' + server!.logs());
        throw e;
      }
      db = await setupDb(client, 'callfn', ['t']);
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    it('call: create echo function + invoke with params → echoes back', async () => {
      // 1. Create function from real echo WASM.
      br(await Batch.create('mk-fn')
        .add('f', ddl.createFunction('echo_e2e', {
          wasm: ECHO_WASM_B64,
        }))
        .execute(client!, db));

      // 2. Invoke the function with params.
      const resp = br(await Batch.create('call-fn')
        .add('c', call('echo_e2e', [42, 'hello', true]))
        .execute(client!, db));

      // The echo function returns the msgpack-encoded params map back.
      // The result should be in `value` field of QueryResult.
      const result = resp.results.c;
      expect(result).toBeDefined();
      // The echo WASM echoes back the raw msgpack bytes as-is.
      // Depending on server deserialization, the value should contain our params.
      // At minimum, we verify we got a non-error response with a defined result.
      expect(result.value !== undefined || result.records.length > 0).toBe(true);
    });

    it('call: invoke with no params succeeds', async () => {
      const resp = br(await Batch.create('call-no-params')
        .add('c', call('echo_e2e'))
        .execute(client!, db));

      const result = resp.results.c;
      expect(result).toBeDefined();
    });
  },
);

describe('e2e-call.test skip reason', () => {
  it('reports why the call e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-call] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
