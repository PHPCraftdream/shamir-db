/**
 * End-to-end proof of FG-5d (TS SDK streaming cursor) over the TS client's
 * real wire round-trip (real server process, real WS/TLS client).
 *
 * Mirrors the Rust SDK's `crates/shamir-client/src/tests/cursor_stream_tests.rs`
 * (FG-5c) — same "prove close() reaches the server" style of assertion,
 * ported to `CursorIterator`'s `for await`/`break`/`close()` idiom.
 *
 * Covers:
 * - Happy path: N rows across 3+ pages at a small `pageSize`, collected via
 *   `for await` in order (ORDER BY-driven).
 * - Early `break`: proves `return()` (invoked automatically by the JS
 *   runtime's `IteratorClose` on `break`) actually reached the server by
 *   driving a raw `fetch_next` against the SAME cursor id afterwards and
 *   asserting `cursor_not_found`.
 * - Explicit `close()` outside a loop: same proof, called manually.
 * - Error propagation: an `AsOf`-temporal query throws `ShamirDbError` with
 *   `code === 'cursor_temporal_not_supported'`.
 * - Empty result set: the `for await` loop body never executes, no error.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, WireValue } from '../index.js';
import { Query, filter, write, ddl, ShamirDbError } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  uniqueDbName,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e cursor streaming (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-cursors] connection failed. Server logs:\n' + server.logs());
        throw e;
      }

      db = uniqueDbName('cursors');
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

    async function seedItems(scenario: string, n: number): Promise<void> {
      const rows: Array<Record<string, WireValue>> = [];
      for (let i = 0; i < n; i += 1) {
        rows.push({ scenario, sku: `k${i.toString().padStart(3, '0')}`, qty: i });
      }
      await client!.execute(db, {
        id: `seed-${db}-${scenario}`,
        queries: { ins: write.insert('items', rows) },
      });
    }

    // ═══════════════════════════════════════════════════════════════════
    // Happy path: 10 rows / page_size 3 -> 4 pages, collected in ORDER BY
    // order via `for await`.
    // ═══════════════════════════════════════════════════════════════════

    it('for await collects all rows across multiple pages in order', async () => {
      await seedItems('happy', 10);

      const query = Query.from('items')
        .where(filter.eq('scenario', 'happy'))
        .orderByAsc('qty')
        .build();

      const collected: number[] = [];
      for await (const record of client!.streamCursor(db, query, 3)) {
        collected.push(record.qty as number);
      }

      expect(collected).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    });

    // ═══════════════════════════════════════════════════════════════════
    // Early break: proves return()/IteratorClose reached the server.
    // ═══════════════════════════════════════════════════════════════════

    it('break mid-iteration cancels the cursor server-side', async () => {
      await seedItems('breakmid', 10);

      const query = Query.from('items')
        .where(filter.eq('scenario', 'breakmid'))
        .orderByAsc('qty')
        .build();

      const cursor = client!.streamCursor(db, query, 2);
      let seen = 0;
      for await (const record of cursor) {
        seen += 1;
        void record;
        if (seen === 1) break;
      }

      expect(seen).toBe(1);
      const cursorId = cursor.cursorId;
      expect(cursorId).toBeDefined();

      // Drive a raw fetch_next against the SAME cursor id — proves break's
      // implicit IteratorClose -> return() -> cancel_cursor actually reached
      // the server (the registry entry for this id is gone).
      let probeErr: unknown;
      try {
        await client!.probeCursorOp({ op: 'fetch_next', cursor_id: cursorId!, page_size: 2 });
      } catch (e) {
        probeErr = e;
      }
      expect(probeErr).toBeInstanceOf(ShamirDbError);
      expect((probeErr as ShamirDbError).code).toBe('cursor_not_found');
    });

    // ═══════════════════════════════════════════════════════════════════
    // Explicit close() outside a loop: same proof, called manually.
    // ═══════════════════════════════════════════════════════════════════

    it('explicit close() outside a loop cancels the cursor server-side', async () => {
      await seedItems('explicitclose', 10);

      const query = Query.from('items')
        .where(filter.eq('scenario', 'explicitclose'))
        .orderByAsc('qty')
        .build();

      const cursor = client!.streamCursor(db, query, 2);
      const first = await cursor.next();
      expect(first.done).toBe(false);

      const cursorId = cursor.cursorId;
      expect(cursorId).toBeDefined();

      await cursor.close();

      let probeErr: unknown;
      try {
        await client!.probeCursorOp({ op: 'fetch_next', cursor_id: cursorId!, page_size: 2 });
      } catch (e) {
        probeErr = e;
      }
      expect(probeErr).toBeInstanceOf(ShamirDbError);
      expect((probeErr as ShamirDbError).code).toBe('cursor_not_found');
    });

    // ═══════════════════════════════════════════════════════════════════
    // Error propagation: AsOf-temporal query throws on first next()/first
    // loop iteration, not a silently-swallowed done: true.
    // ═══════════════════════════════════════════════════════════════════

    it('temporal AsOf query throws ShamirDbError(cursor_temporal_not_supported)', async () => {
      await seedItems('temporal', 3);

      const query = Query.from('items')
        .where(filter.eq('scenario', 'temporal'))
        .asOfVersion(1)
        .build();

      const cursor = client!.streamCursor(db, query, 10);

      let thrown: unknown;
      try {
        for await (const record of cursor) {
          void record;
        }
      } catch (e) {
        thrown = e;
      }

      expect(thrown).toBeInstanceOf(ShamirDbError);
      expect((thrown as ShamirDbError).code).toBe('cursor_temporal_not_supported');
    });

    // ═══════════════════════════════════════════════════════════════════
    // Empty result set: the loop body never executes, no error.
    // ═══════════════════════════════════════════════════════════════════

    it('empty result set yields no records and no error', async () => {
      const query = Query.from('items')
        .where(filter.eq('scenario', 'no-such-scenario-at-all'))
        .build();

      let iterations = 0;
      for await (const record of client!.streamCursor(db, query, 5)) {
        void record;
        iterations += 1;
      }

      expect(iterations).toBe(0);
    });

    it('reports why the e2e-cursors test was skipped', () => {
      if (!SERVER_AVAILABLE) {
        console.warn(
          '[e2e-cursors] SKIPPED: release shamir-server binary not found. ' +
            'Build it with: cargo build --release -p shamir-server',
        );
      }
      expect(true).toBe(true);
    });
  },
);
