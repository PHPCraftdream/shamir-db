/**
 * End-to-end proof of OQL Epic 04 `for_each` data-dependent loop over the
 * TS client's real wire round-trip (real server process, real WS/TLS
 * client).
 *
 * Mirrors `crates/shamir-client/tests/batch_for_each_e2e.rs` — same
 * scenarios, ported to the TS builder (`Batch.forEach`, task #654).
 *
 * Unlike Epic03's `when` e2e twin (`e2e-when.test.ts`, which hit a real
 * blocking bug — field-based comparisons always fold to a fixed result,
 * #651, still open, forcing that file to use synthetic `isNull`/`isNotNull`
 * guards instead of real data-driven conditions), `for_each`'s `over` has
 * NO such limitation: `over` can be a genuine `$query` column reference,
 * resolved once against real data, with no scratch-interner involved (see
 * `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`'s "Bug #651 —
 * independence of `bind_row`" section). So this file exercises the REAL,
 * INTENDED, canonical scenario directly — no workaround needed for that
 * part.
 *
 * # NOTED ENGINE GAP — found while writing the Rust twin of this file
 * (`batch_for_each_e2e.rs`), NOT fixed here, same out-of-scope rule as
 * `e2e-when.test.ts`'s header
 *
 * A transactional batch whose ONLY top-level data-bearing entry is a bare
 * `for_each` (no other `Read`/`Insert`/etc. at the top level) fails with
 * `"transactional batch has no data ops to target a repo"`
 * (`crates/shamir-engine/src/query/batch/batch_execute.rs:449`), because
 * `distinct_repos()` (`crates/shamir-query-types/src/batch/query_entry.rs:90`)
 * determines the tx's repo purely via `BatchOp::table_ref()`
 * (`crates/shamir-query-types/src/batch/batch_op.rs:561-573`), which returns
 * `None` for both `BatchOp::Batch` and `BatchOp::ForEach` — it does NOT walk
 * into the nested body's `queries` map. This directly contradicts the
 * Epic04 ADR's own stated requirement (see the Rust twin's doc comment for
 * the exact quote). Worked around here (in the literal-array scenario,
 * which otherwise has no other top-level data op) by adding a harmless
 * top-level read alongside the `forEach` — this does not touch or weaken
 * what that scenario actually proves. Track the real fix under a dedicated
 * follow-up task, not Epic04/E.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse } from '../index.js';
import { Query, Batch, filter, write, ddl } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

/**
 * `QueryResult.skipped` defaults to `false` and is omitted from the wire
 * entirely when false (`crates/shamir-query-types/src/read/query_result.rs:85-90`),
 * so a `for_each` entry that actually ran carries no `skipped` key at all
 * — same observed shape documented in `e2e-when.test.ts`'s header for the
 * `when`/`switchCase` primitives. Treat "missing" as "not skipped".
 */
function isSkipped(result: unknown): boolean {
  return (result as { skipped?: boolean }).skipped === true;
}

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e for_each data-dependent loop (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-for-each] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
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

    async function makeDb(db: string): Promise<void> {
      await client!.execute('default', {
        id: `mk-db-${db}`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `mk-tables-${db}`,
        queries: {
          mr: ddl.createRepo('main'),
          tbOrders: ddl.createTable('orders', { repo: 'main' }),
          tbAudit: ddl.createTable('audit_log', { repo: 'main' }),
        },
      });
    }

    /** Loop body: insert one `audit_log` row, `order_id` bound to the
     * current loop element via `{ "$param": bindRow }`. */
    function auditInsertBody(bindRow: string): Batch {
      const inner = Batch.create('inner');
      inner.add(
        'audit',
        write.insert('audit_log', { order_id: { $param: bindRow }, note: 'audited' }),
      );
      return inner;
    }

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 1 (canonical): read order ids for a customer via a real
    // $query column ref, then for_each-insert one audit_log row per order.
    // ═══════════════════════════════════════════════════════════════════

    it('for_each over a $query column-ref inserts one audit row per real order over the real wire', async () => {
      const db = uniqueDbName('fe_basic');
      await makeDb(db);

      const expectedIds = [1001, 1002, 1003];
      await client!.execute(db, {
        id: `seed-${db}`,
        queries: {
          o1: write.insert('orders', {
            order_id: expectedIds[0],
            customer_id: 'alice',
            amount: 10,
          }),
          o2: write.insert('orders', {
            order_id: expectedIds[1],
            customer_id: 'alice',
            amount: 20,
          }),
          o3: write.insert('orders', {
            order_id: expectedIds[2],
            customer_id: 'alice',
            amount: 30,
          }),
          o4: write.insert('orders', {
            order_id: 2001,
            customer_id: 'bob',
            amount: 99,
          }),
        },
      });

      const batch = Batch.create('txn-basic');
      batch.add('orders_q', Query.from('orders').where(filter.eq('customer_id', 'alice')).build());
      const overRef = batch.handle('orders_q').column('order_id');
      batch.forEach('loop', overRef, 'order_id', auditInsertBody('order_id'));
      batch.transactional();

      const resp: BatchResponse = br(await client!.execute(db, batch.build()));

      const loopResult = resp.results.loop as { skipped?: boolean; value?: unknown };
      expect(isSkipped(loopResult)).toBe(false);
      const list = loopResult.value as unknown[];
      expect(list).toHaveLength(3);

      const verify: BatchResponse = br(
        await client!.execute(db, {
          id: `verify-${db}`,
          queries: { audit_rows: Query.from('audit_log').build() },
        }),
      );
      const rows = verify.results.audit_rows.records as Array<{ order_id: number; note: string }>;
      expect(rows).toHaveLength(3);
      const actualIds = rows.map((r) => r.order_id).sort((a, b) => a - b);
      expect(actualIds).toEqual(expectedIds);
      for (const row of rows) {
        expect(row.note).toBe('audited');
      }
    });

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 2: zero iterations.
    // ═══════════════════════════════════════════════════════════════════

    it('for_each over zero matching orders produces an empty list and no audit rows over the real wire', async () => {
      const db = uniqueDbName('fe_zero');
      await makeDb(db);

      await client!.execute(db, {
        id: `seed-${db}`,
        queries: {
          o1: write.insert('orders', { order_id: 1, customer_id: 'dave', amount: 5 }),
        },
      });

      const batch = Batch.create('txn-zero');
      batch.add('orders_q', Query.from('orders').where(filter.eq('customer_id', 'carol')).build());
      const overRef = batch.handle('orders_q').column('order_id');
      batch.forEach('loop', overRef, 'order_id', auditInsertBody('order_id'));
      batch.transactional();

      const resp: BatchResponse = br(await client!.execute(db, batch.build()));

      const loopResult = resp.results.loop as { skipped?: boolean; value?: unknown };
      expect(isSkipped(loopResult)).toBe(false);
      const list = loopResult.value as unknown[];
      expect(list).toHaveLength(0);

      const verify: BatchResponse = br(
        await client!.execute(db, {
          id: `verify-${db}`,
          queries: { audit_rows: Query.from('audit_log').build() },
        }),
      );
      const rows = verify.results.audit_rows.records as unknown[];
      expect(rows).toHaveLength(0);
    });

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 3: literal-array `over`.
    // ═══════════════════════════════════════════════════════════════════

    it('for_each over a literal array inserts one audit row per literal over the real wire', async () => {
      const db = uniqueDbName('fe_literal');
      await makeDb(db);

      const batch = Batch.create('txn-literal');
      // Harmless top-level read so `distinct_repos()` can find a
      // `table_ref()` — see this file's header for the engine gap this
      // works around.
      batch.add('orders_probe', Query.from('orders').build());
      batch.forEach('loop', [101, 202, 303], 'order_id', auditInsertBody('order_id'));
      batch.transactional();

      const resp: BatchResponse = br(await client!.execute(db, batch.build()));

      const loopResult = resp.results.loop as { skipped?: boolean; value?: unknown };
      expect(isSkipped(loopResult)).toBe(false);
      const list = loopResult.value as unknown[];
      expect(list).toHaveLength(3);

      const verify: BatchResponse = br(
        await client!.execute(db, {
          id: `verify-${db}`,
          queries: { audit_rows: Query.from('audit_log').build() },
        }),
      );
      const rows = verify.results.audit_rows.records as Array<{ order_id: number }>;
      expect(rows).toHaveLength(3);
      const actualIds = rows.map((r) => r.order_id).sort((a, b) => a - b);
      expect(actualIds).toEqual([101, 202, 303]);
    });

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 4: error mid-loop in a transactional batch rolls back the
    // whole tx (a "good" write before the loop, plus the loop's own
    // partial writes).
    // ═══════════════════════════════════════════════════════════════════

    it('for_each iteration error mid-loop rolls back the whole transactional batch over the real wire', async () => {
      const db = uniqueDbName('fe_txabort');
      await makeDb(db);

      await client!.execute(db, {
        id: `mk-index-${db}`,
        queries: {
          ix: ddl.createIndex('audit_log_order_id_uq', 'audit_log', [['order_id']], {
            unique: true,
            repo: 'main',
          }),
        },
      });

      const batch = Batch.create('txn-abort');
      batch.add('good', write.insert('orders', { order_id: 1, customer_id: 'erin', amount: 1 }));
      // over: [1, 1, 2] -- iteration 0 inserts order_id=1 (ok), iteration 1
      // duplicates order_id=1 (unique-index violation), iteration 2 must
      // never run.
      batch.forEach('loop', [1, 1, 2], 'order_id', auditInsertBody('order_id'));
      batch.transactional();

      let resp: BatchResponse | undefined;
      let caught: unknown;
      try {
        resp = br(await client!.execute(db, batch.build()));
      } catch (e) {
        caught = e;
      }

      if (resp) {
        const txInfo = resp.transaction as { status: string } | undefined;
        expect(txInfo).toBeDefined();
        expect(txInfo!.status).toBe('aborted');
      } else {
        // The transport-level error itself is proof the batch did not
        // commit; the row-count assertions below confirm no partial
        // writes survived.
        expect(caught).toBeDefined();
      }

      const verify: BatchResponse = br(
        await client!.execute(db, {
          id: `verify-${db}`,
          queries: {
            orders_rows: Query.from('orders').build(),
            audit_rows: Query.from('audit_log').build(),
          },
        }),
      );
      const orderRows = verify.results.orders_rows.records as unknown[];
      const auditRows = verify.results.audit_rows.records as unknown[];
      expect(orderRows).toHaveLength(0);
      expect(auditRows).toHaveLength(0);
    });

    it('reports why the e2e-for-each test was skipped', () => {
      if (!SERVER_AVAILABLE) {
        console.warn(
          '[e2e-for-each] SKIPPED: release shamir-server binary not found. ' +
            'Build it with: cargo build --release -p shamir-server',
        );
      }
      expect(true).toBe(true);
    });
  },
);
