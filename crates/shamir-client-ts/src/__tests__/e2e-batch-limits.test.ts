/**
 * End-to-end proof that `Batch.limits()` is wire-compatible with the
 * REAL Rust server (#662).
 *
 * Bug recap: `BatchLimits.max_iterations` (Epic04/B, #653) was added on the
 * Rust side WITHOUT `#[serde(default = ...)]`, making the field mandatory on
 * deserialization whenever a `limits` map is present on the wire. The TS
 * client's `BatchLimits` interface / `DEFAULT_LIMITS` / `.limits()` builder
 * were never updated for #653 — they only knew the original 5 fields. Any
 * TS client calling `.limits(partial)` therefore sent a 5-field map that the
 * server rejected with `"missing field \`max_iterations\`"`.
 *
 * Only shape-level unit tests existed before this file (`batch.test.ts`),
 * and none of them round-tripped through the Rust server's actual serde
 * deserializer — so the break shipped silently. This file closes that gap
 * by running `.limits()` against a REAL server process.
 *
 * Fix (both sides, see brief
 * `docs/dev-artifacts/prompts/bugfix-662/01-max-iterations-serde-default-ts-parity.md`):
 *   - Rust: `BatchLimits.max_iterations` gained
 *     `#[serde(default = "default_max_iterations")]` (same `1000` value as
 *     `BatchLimits::default()`), so a `limits` map omitting the field still
 *     deserializes.
 *   - TS: `BatchLimits` interface / `DEFAULT_LIMITS` / `.limits()` now know
 *     about `max_iterations` (6 fields total).
 *
 * This file proves BOTH directions still work over the real wire:
 *   1. `.limits({...})` deliberately omitting `max_iterations` — the TS
 *      builder itself now always fills it (so a "TS client that doesn't
 *      know the field" can no longer be expressed via `.limits()` alone);
 *      to genuinely exercise the Rust-side default, this test posts a raw
 *      5-field limits object directly (bypassing the builder), simulating
 *      exactly the older/naive client shape #662 was root-caused against.
 *   2. `.limits({ max_iterations: N })` — explicit value round-trips.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse } from '../index.js';
import { Query, Batch, write, ddl } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e BatchLimits.max_iterations wire compatibility (#662, requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-batch-limits] connection failed. Server logs:\n' + server.logs());
        throw e;
      }

      db = uniqueDbName('lim');
      await client.execute('default', {
        id: `mk-db-${db}`,
        queries: { mk: ddl.createDb(db) },
      });
      await client.execute(db, {
        id: `mk-tables-${db}`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('items', { repo: 'main' }),
        },
      });
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try {
          await client.close();
        } catch {
          /* ok */
        }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    it('a limits map missing max_iterations still succeeds against the real server (Rust-side default kicks in)', async () => {
      // Deliberately bypass the TS builder's `.limits()` (which now always
      // fills all 6 fields) to reproduce the exact 5-field wire shape an
      // older/naive client (or the pre-#662 TS builder) would have sent.
      const req = Batch.create(`no-max-iter-${db}`)
        .add('ins', write.insert('items', { sku: 'LIM-1', qty: 1 }))
        .build();
      (req as unknown as { limits: object }).limits = {
        max_queries: 50,
        max_dependency_depth: 10,
        max_execution_time_secs: 30,
        max_result_size: 10_485_760,
        max_nesting_depth: 4,
        // max_iterations intentionally omitted.
      };

      const resp: BatchResponse = br(await client!.execute(db, req));
      expect(resp.results.ins).toBeDefined();

      const check = br(
        await client!.execute(db, {
          id: `chk-no-max-iter-${db}`,
          queries: { rd: Query.from('items').build() },
        }),
      );
      expect(
        check.results.rd.records.some((r) => r.sku === 'LIM-1'),
      ).toBe(true);
    });

    it('.limits({ max_iterations }) explicitly set round-trips through the real server', async () => {
      const batch = Batch.create(`explicit-max-iter-${db}`)
        .add('ins', write.insert('items', { sku: 'LIM-2', qty: 2 }))
        .limits({ max_iterations: 7 });

      const resp = await batch.execute(client!, db);
      expect(resp.results.ins).toBeDefined();

      const check = br(
        await client!.execute(db, {
          id: `chk-explicit-max-iter-${db}`,
          queries: { rd: Query.from('items').build() },
        }),
      );
      expect(
        check.results.rd.records.some((r) => r.sku === 'LIM-2'),
      ).toBe(true);
    });
  },
);
