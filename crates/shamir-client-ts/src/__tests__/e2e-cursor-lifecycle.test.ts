/**
 * FG-5e — the two cursor-contour gaps NOT already covered by FG-5a/b/c/d's
 * own test suites, observed through the real TS SDK (`ShamirClient.streamCursor`)
 * against a real server process:
 *
 * - Idle-timeout eviction, proven through `streamCursor`/`probeCursorOp`
 *   rather than the server-side registry/reaper directly (FG-5b already
 *   covers the registry level in `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`).
 * - Per-session open-cursor cap rejection, proven through `streamCursor`
 *   rather than the registry's cap logic directly — including that a
 *   second session (a second `connectAdmin` connection) is unaffected by
 *   the first session's cap.
 *
 * Everything else in the "full cursor contour" (happy-path pagination,
 * break/close mid-stream reaching the server, AsOf/empty-result error
 * propagation) is already proven by `e2e-cursors.test.ts` (FG-5d) — this
 * file does not duplicate any of that, hence the distinct file name.
 *
 * The background reaper sweep interval (`DEFAULT_CURSOR_REAPER_INTERVAL`,
 * 5s) is hardcoded server-side, not configurable — only `idleTimeoutSecs`
 * is. So the idle-timeout test below sleeps past BOTH the configured idle
 * window AND a full reaper sweep, not just the idle window alone.
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

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e cursor lifecycle: idle-timeout + per-session cap (requires release binary)',
  () => {
    // ═══════════════════════════════════════════════════════════════════
    // Idle-timeout eviction.
    // ═══════════════════════════════════════════════════════════════════

    describe('idle-timeout eviction', () => {
      let server: ServerHandle | null = null;
      let client: ShamirClient | null = null;
      let db: string;

      beforeAll(async () => {
        server = await startServer({ cursors: { idleTimeoutSecs: 1 } });
        try {
          client = await connectAdmin(HOST, server.port);
        } catch (e) {
          console.error('[e2e-cursor-lifecycle] connection failed. Server logs:\n' + server.logs());
          throw e;
        }

        db = uniqueDbName('cursor_idle');
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

      it(
        'cursor left un-fetched for idleTimeoutSecs + a reaper sweep is evicted with cursor_expired',
        async () => {
          await seedItems('idle', 10);

          const query = Query.from('items')
            .where(filter.eq('scenario', 'idle'))
            .orderByAsc('qty')
            .build();

          const cursor = client!.streamCursor(db, query, 2);
          const first = await cursor.next();
          expect(first.done).toBe(false);

          const cursorId = cursor.cursorId;
          expect(cursorId).toBeDefined();

          // Idle window (1s) + one full reaper sweep (hardcoded 5s) + slack.
          await sleep(7_000);

          let probeErr: unknown;
          try {
            await client!.probeCursorOp({ op: 'fetch_next', cursor_id: cursorId!, page_size: 2 });
          } catch (e) {
            probeErr = e;
          }
          expect(probeErr).toBeInstanceOf(ShamirDbError);
          expect((probeErr as ShamirDbError).code).toBe('cursor_expired');
        },
        15_000,
      );

      it('reports why the idle-timeout test was skipped', () => {
        if (!SERVER_AVAILABLE) {
          console.warn(
            '[e2e-cursor-lifecycle] SKIPPED: release shamir-server binary not found. ' +
              'Build it with: cargo build --release -p shamir-server',
          );
        }
        expect(true).toBe(true);
      });
    });

    // ═══════════════════════════════════════════════════════════════════
    // Per-session open-cursor cap rejection.
    // ═══════════════════════════════════════════════════════════════════

    describe('per-session open-cursor cap', () => {
      let server: ServerHandle | null = null;
      let client: ShamirClient | null = null;
      let otherClient: ShamirClient | null = null;
      let db: string;

      beforeAll(async () => {
        server = await startServer({ cursors: { maxCursorsPerSession: 2 } });
        try {
          client = await connectAdmin(HOST, server.port);
        } catch (e) {
          console.error('[e2e-cursor-lifecycle] connection failed. Server logs:\n' + server.logs());
          throw e;
        }

        db = uniqueDbName('cursor_cap');
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
        if (otherClient) {
          try {
            await otherClient.close();
          } catch {
            /* ok */
          }
          otherClient = null;
        }
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

      it('a 3rd cursor is rejected with cursor_limit_exceeded, other sessions unaffected', async () => {
        await seedItems('cap', 10);

        const query = () =>
          Query.from('items')
            .where(filter.eq('scenario', 'cap'))
            .orderByAsc('qty')
            .build();

        // Cursor 1: open + `.next()` once so `create_cursor` round-trips.
        const cursor1 = client!.streamCursor(db, query(), 2);
        const first1 = await cursor1.next();
        expect(first1.done).toBe(false);

        // Cursor 2: same.
        const cursor2 = client!.streamCursor(db, query(), 2);
        const first2 = await cursor2.next();
        expect(first2.done).toBe(false);

        // Cursor 3: over the cap of 2 -> first `.next()` must throw.
        const cursor3 = client!.streamCursor(db, query(), 2);
        let thrown: unknown;
        try {
          await cursor3.next();
        } catch (e) {
          thrown = e;
        }
        expect(thrown).toBeInstanceOf(ShamirDbError);
        expect((thrown as ShamirDbError).code).toBe('cursor_limit_exceeded');

        // A different session (a second `connectAdmin` connection) must be
        // unaffected — it can open its own cursor successfully, proving
        // the cap is per-session, not global.
        otherClient = await connectAdmin(HOST, server!.port);
        const otherCursor = otherClient.streamCursor(db, query(), 2);
        const otherFirst = await otherCursor.next();
        expect(otherFirst.done).toBe(false);
        await otherCursor.close();
      });

      it('reports why the per-session cap test was skipped', () => {
        if (!SERVER_AVAILABLE) {
          console.warn(
            '[e2e-cursor-lifecycle] SKIPPED: release shamir-server binary not found. ' +
              'Build it with: cargo build --release -p shamir-server',
          );
        }
        expect(true).toBe(true);
      });
    });
  },
);
