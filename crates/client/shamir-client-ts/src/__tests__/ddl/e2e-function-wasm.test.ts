/**
 * End-to-end test of the wasm function lifecycle: create with optimize,
 * describe, export, check, alter.
 *
 * Result record shapes follow the PROC-3b wire contract; this test has not
 * been executed against a server at the time of writing.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../../index.js';
import { Batch, ddl } from '../../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  setupDb,
} from '../e2e-harness.js';
import type { ServerHandle } from '../e2e-harness.js';

/** Echo WASM module (same as e2e-call.test.ts). */
const ECHO_WASM_B64 =
  'AGFzbQEAAAABDAJgAX8Bf2ACf38BfgMDAgABBQMBAAIGBwF/AUGACAsHJwMGbWVtb3J5AgAMc2hhbWlyX2FsbG9jAAALc2hhbWlyX2NhbGwAAQogAhEBAX8jACEBIwAgAGokACABCwwAIACtQiCGIAGthAsAKQRuYW1lAhkCAAIAA2xlbgEDcHRyAQIAA3B0cgEDbGVuBwcBAARidW1w';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e function wasm — describe/export/check/alter (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;
    let describedHash: string;

    const describeRecord = async (): Promise<Record<string, unknown>> => {
      const resp = br(await Batch.create('desc-fn')
        .add('d', ddl.describeFunction('echo_fn'))
        .execute(client!, db));
      return resp.results.d.records[0] as Record<string, unknown>;
    };

    const listJson = async (): Promise<string> => {
      const resp = br(await Batch.create('ls-fn')
        .add('l', ddl.listFunctions())
        .execute(client!, db));
      return JSON.stringify(resp.results.l.records);
    };

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-function-wasm] connection failed. Server logs:\n' + server!.logs());
        throw e;
      }
      db = await setupDb(client, 'fnwasm', ['t']);
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

    it('create echo function with optimize never, then describe', async () => {
      br(await Batch.create('mk-fn')
        .add('f', ddl.createFunction('echo_fn', {
          wasm: ECHO_WASM_B64,
          optimize: 'never',
        }))
        .execute(client!, db));

      const rec = await describeRecord();
      expect(rec.name).toBe('echo_fn');
      expect(rec.lang).toBe('wasm');
      expect(rec.optimize).toBe('never');
      expect(rec.hash).toMatch(/^[0-9a-f]{32}$/);
      expect((rec.state as { kind: string }).kind).toBe('loaded');
      describedHash = rec.hash as string;
    });

    it('exportFunction round-trips the wasm', async () => {
      const resp = br(await Batch.create('exp-fn')
        .add('e', ddl.exportFunction('echo_fn'))
        .execute(client!, db));
      const rec = resp.results.e.records[0] as Record<string, unknown>;
      expect(rec.wasm).toBe(ECHO_WASM_B64);
      expect(rec.hash).toBe(describedHash);
    });

    it('checkFunction validates without persisting', async () => {
      const before = await listJson();

      const resp = br(await Batch.create('chk-fn')
        .add('c', ddl.checkFunction({ wasm: ECHO_WASM_B64 }))
        .execute(client!, db));
      const rec = resp.results.c.records[0] as Record<string, unknown>;
      expect(rec.ok).toBe(true);
      expect(rec.hash).toBe(describedHash);

      const after = await listJson();
      expect(after).toBe(before);
    });

    it('alterFunction changes optimize', async () => {
      br(await Batch.create('alt-fn')
        .add('a', ddl.alterFunction('echo_fn', 'now'))
        .execute(client!, db));

      const rec = await describeRecord();
      expect(rec.optimize).toBe('now');
    });
  },
);

describe('e2e-function-wasm.test skip reason', () => {
  it('reports why the function wasm e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-function-wasm] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
