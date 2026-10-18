/**
 * End-to-end tests — typed builders against a live shamir-server.
 *
 * Spawns a single server + connects a single client in `beforeAll`.
 * Each `it` exercises one scenario ported from the JS e2e suite
 * (tests/e2e/tests/02–08, 12, 15) but uses ONLY the typed TS builders.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse, WireValue } from '../../index.js';
import {
  Query,
  Batch,
  filter,
  select,
  write,
  ddl,
} from '../../index.js';
import {
  SERVER_BIN,
  SERVER_AVAILABLE,
  HOST,
  ORIGIN,
  startServer,
  connectAdmin,
  connectAs,
  br,
  uniqueDbName,
  setupDb,
  seed,
} from '../e2e-harness.js';
import type { ServerHandle } from '../e2e-harness.js';

// ─── test suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e typed builders (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let PORT = 0;

    beforeAll(async () => {
      server = await startServer();
      PORT = server.port;
      try {
        client = await connectAdmin(HOST, PORT);
      } catch (e) {
        console.error('[e2e.test] connection failed. Server logs:\n' + server.logs());
        throw e;
      }
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
    // ── 8. Admin DDL + HMAC (port 08/12) ────────────────────────────────

    it('DDL: list databases includes default', async () => {
      const resp = br(await Batch.create('lsdb')
        .add('l', ddl.listDatabases())
        .execute(client!, 'default'));
      const names = resp.results.l.records[0].databases as string[];
      expect(Array.isArray(names)).toBe(true);
      expect(names).toContain('default');
    });

    it('DDL: create_index + list + drop_index (hmac)', async () => {
      const idxDb = await setupDb(client!, 'ddl_idx', ['t']);

      await br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('by_email', 't', [['email']]))
        .execute(client!, idxDb));

      const lsResp = br(await Batch.create('ls-idx')
        .add('l', ddl.listIndexes('t'))
        .execute(client!, idxDb));
      const indexNames = (lsResp.results.l.records[0].indexes as Array<{ name: string }>).map(i => i.name);
      expect(indexNames).toContain('by_email');

      // Drop with HMAC — client IS the HmacSigner
      await br(await Batch.create('rm-idx')
        .add('d', ddl.dropIndex(client!, idxDb, 'main', 't', 'by_email'))
        .execute(client!, idxDb));

      const ls2 = br(await Batch.create('ls-idx2')
        .add('l', ddl.listIndexes('t'))
        .execute(client!, idxDb));
      const afterNames = (ls2.results.l.records[0].indexes as Array<{ name: string }>).map(i => i.name);
      expect(afterNames).not.toContain('by_email');
    });

    it('HMAC: drop_table without hmac -> hmac_required', async () => {
      const hmacDb = await setupDb(client!, 'hmac_miss', ['t']);
      await expect(
        client!.execute(hmacDb, {
          id: 1,
          queries: { d: { drop_table: 't', repo: 'main' } },
        }),
      ).rejects.toThrow(/hmac_required/);
    });

    it('HMAC: drop_table with wrong hmac -> hmac_mismatch', async () => {
      const hmacDb = await setupDb(client!, 'hmac_wrong', ['t']);
      await expect(
        client!.execute(hmacDb, {
          id: 1,
          queries: {
            d: {
              drop_table: 't',
              repo: 'main',
              hmac: 'aa'.repeat(32),
            },
          },
        }),
      ).rejects.toThrow(/hmac_mismatch/);
    });

    it('HMAC: drop_table with correct hmac succeeds', async () => {
      const hmacDb = await setupDb(client!, 'hmac_ok', ['t']);
      const resp = br(await Batch.create('drop-ok')
        .add('d', ddl.dropTable(client!, hmacDb, 'main', 't'))
        .execute(client!, hmacDb));
      const row = resp.results.d.records[0] as Record<string, unknown>;
      expect(row.dropped_table).toBe('t');
      expect(row.existed).toBe(true);
    });

    it('HMAC: drop_db without hmac -> hmac_required', async () => {
      const victim = await setupDb(client!, 'hmac_miss_db', []);
      await expect(
        client!.execute('default', {
          id: 1,
          queries: { d: { drop_db: victim } },
        }),
      ).rejects.toThrow(/hmac_required/);
    });

    it('HMAC: drop_db with correct hmac + cascade succeeds', async () => {
      const victim = await setupDb(client!, 'hmac_ok_db', []);
      const resp = br(await Batch.create('drop-db-ok')
        .add('d', ddl.dropDb(client!, victim, { cascade: true }))
        .execute(client!, 'default'));
      const row = resp.results.d.records[0] as Record<string, unknown>;
      expect(row.dropped).toBe(victim);
    });

    // ── 9. Transactions (port 15) ───────────────────────────────────────

    let txDb: string;

    it('tx: setup items + logs', async () => {
      txDb = await setupDb(client!, 'tx_e2e', ['items', 'logs']);
    });

    it('tx: transactional insert + read returns committed data', async () => {
      const ins = br(await Batch.create('tx-si-1-ins')
        .add('ins', write.insert('items', [{ name: 'widget', qty: 10 }]))
        .transactional()
        .execute(client!, txDb));
      expect(ins.transaction).toBeDefined();
      expect(ins.transaction!.status).toBe('committed');
      expect(ins.transaction!.tx_id).toBeGreaterThan(0);
      expect(ins.transaction!.commit_version).toBeGreaterThan(0);
      expect(ins.results.ins.records.length).toBeGreaterThanOrEqual(1);

      const readResp = br(await Batch.create('tx-si-1-read')
        .add('read', Query.from('items'))
        .execute(client!, txDb));
      const recs = readResp.results.read.records;
      expect(recs.length).toBeGreaterThanOrEqual(1);
      const names = recs.map(r => r.name);
      expect(names).toContain('widget');
    });

    it('tx: FG-3 read-your-own-writes — read in the SAME transactional batch sees the just-inserted row', async () => {
      // Mirrors the Rust e2e `interactive_tx_happy_path_wire` RYOW
      // assertion: within ONE transactional batch, an insert followed by a
      // read (both ops sharing the SAME TxContext, not yet committed) must
      // see the staged-but-uncommitted row. Before FG-3, streaming scans did
      // not overlay the tx's own write_set, so this read would have missed
      // the row inserted earlier in the same batch.
      const resp = br(await Batch.create('tx-ryow')
        .add('ins', write.insert('items', [{ name: 'ryow-widget', qty: 42 }]))
        .add('read', Query.from('items').where(filter.eq('name', 'ryow-widget')))
        .transactional()
        .execute(client!, txDb));
      expect(resp.transaction!.status).toBe('committed');
      const recs = resp.results.read.records;
      expect(recs.length).toBe(1);
      expect(recs[0].name).toBe('ryow-widget');
      expect(recs[0].qty).toBe(42);
    });

    it('tx: cross-table insert is atomic', async () => {
      const resp = br(await Batch.create('tx-cross-table')
        .add('ins_items', write.insert('items', [{ name: 'cross-item' }]))
        .add('ins_logs', write.insert('logs', [{ event: 'item_created' }]))
        .transactional()
        .execute(client!, txDb));
      expect(resp.transaction!.status).toBe('committed');
      expect(resp.results.ins_items.records.length).toBeGreaterThanOrEqual(1);
      expect(resp.results.ins_logs.records.length).toBeGreaterThanOrEqual(1);
    });

    it('tx: isolation serializable accepted', async () => {
      const resp = br(await Batch.create('tx-ssi')
        .add('ins', write.insert('items', [{ name: 'ssi-item' }]))
        .transactional('serializable')
        .execute(client!, txDb));
      expect(resp.transaction!.status).toBe('committed');
    });

    it('tx: non-tx insert works alongside tx infra', async () => {
      const resp = br(await Batch.create('non-tx')
        .add('ins', write.insert('items', [{ name: 'plain-item' }]))
        .execute(client!, txDb));
      expect(!resp.transaction || resp.transaction === undefined).toBe(true);
      expect(resp.results.ins.records.length).toBeGreaterThanOrEqual(1);
    });

    // ── 10. Interactive (multi-call) transactions (TS-T9) ───────────────

    let itxDb: string;

    it('itx: setup table', async () => {
      itxDb = await setupDb(client!, 'itx', ['acct']);
    });

    it('itx: begin → execute(write) → commit, row visible after commit', async () => {
      const opened = await client!.txBegin(itxDb, 'main');
      expect(typeof opened.tx_handle).toBe('number');
      expect(opened.snapshot_version).toBeGreaterThanOrEqual(0);
      expect(opened.isolation).toBe('snapshot');

      let committed = false;
      try {
        // Write inside the open tx — accumulates in the parked transaction.
        const insResp = await client!.txExecute(
          itxDb,
          opened.tx_handle,
          Batch.create('itx-ins').add('i', write.insert('acct', [{ id: 'a', bal: 100 }])).build(),
        );
        // Per-call response carries no commit outcome yet (tx still open).
        expect(insResp.transaction === undefined || insResp.transaction === null).toBe(true);

        const info = await client!.txCommit(itxDb, opened.tx_handle);
        committed = true;
        expect(info.status).toBe('committed');
        expect(info.commit_version).toBeGreaterThan(0);
      } finally {
        if (!committed) {
          await client!.txRollback(itxDb, opened.tx_handle).catch(() => {});
        }
      }

      // After commit the row is visible to a fresh non-tx read.
      const after = br(await Batch.create('itx-after')
        .add('r', Query.from('acct'))
        .execute(client!, itxDb));
      expect(after.results.r.records.map(x => x.id)).toContain('a');
    });

    it('itx: rollback discards the writes', async () => {
      const opened = await client!.txBegin(itxDb, 'main');
      let done = false;
      try {
        await client!.txExecute(
          itxDb,
          opened.tx_handle,
          Batch.create('itx-roll-ins').add('i', write.insert('acct', [{ id: 'ghost', bal: 1 }])).build(),
        );
        await client!.txRollback(itxDb, opened.tx_handle);
        done = true;
      } finally {
        if (!done) {
          await client!.txRollback(itxDb, opened.tx_handle).catch(() => {});
        }
      }

      const after = br(await Batch.create('itx-roll-check')
        .add('r', Query.from('acct'))
        .execute(client!, itxDb));
      expect(after.results.r.records.map(x => x.id)).not.toContain('ghost');
    });

    // ── 11. CreateScramUser (TS-T10) ────────────────────────────────────

    it('scram: create a login-capable user, then authenticate as them', async () => {
      const uname = `e2e_user_${process.pid}_${Date.now()}`;
      const upass = 'another correct horse battery staple';

      const created = await client!.createScramUser(uname, upass, []);
      expect(created.name).toBe(uname);
      expect(created.user_id.length).toBe(16);

      // The freshly-created user can complete a full SCRAM handshake.
      const user2 = await connectAs(HOST, PORT, uname, upass);
      try {
        expect(user2.sessionId().length).toBe(32);
      } finally {
        await user2.close();
      }
    });

    // ── 12. Bound Db handle (TS-T14) ────────────────────────────────────

    let handleDb: string;

    it('handle: setup via db.run(ddl.createTable)', async () => {
      const db = uniqueDbName('handle');
      // Create database via Layer-1
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      // Switch to handle
      const app = client!.db(db);
      await app.run(ddl.createRepo('main'));
      await app.run(ddl.createTable('items', { repo: 'main' }));
      handleDb = db;
    });

    it('handle: db.run(write.insert) + db.query().rows()', async () => {
      const app = client!.db(handleDb);
      await app.run(write.insert('items', [{ id: 'H1', qty: 10 }]));

      const rows = await app.query('items').where(filter.eq('id', 'H1')).rows();
      expect(rows.length).toBe(1);
      expect(rows[0].id).toBe('H1');
      expect(rows[0].qty).toBe(10);
    });

    it('handle: db.query().ex() returns QueryResult', async () => {
      const app = client!.db(handleDb);
      const qr = await app.query('items').where(filter.eq('id', 'H1')).ex();
      expect(qr.records.length).toBe(1);
      expect(qr.records[0].qty).toBe(10);
    });

    it('handle: db.run(write.update) + verify', async () => {
      const app = client!.db(handleDb);
      await app.run(write.update('items').where(filter.eq('id', 'H1')).set({ qty: 99 }));
      const rows = await app.query('items').where(filter.eq('id', 'H1')).rows();
      expect(rows[0].qty).toBe(99);
    });

    it('handle: db.batch().add(...).transactional().run()', async () => {
      const app = client!.db(handleDb);
      const resp = await app
        .batch('tx-handle')
        .add('ins', write.insert('items', [{ id: 'H2', qty: 5 }]))
        .transactional()
        .run();
      expect(resp.transaction).toBeDefined();
      expect(resp.transaction!.status).toBe('committed');
    });

    it('handle: db.batch().add(query).run() with bound query', async () => {
      const app = client!.db(handleDb);
      const resp = await app
        .batch()
        .add('all', app.query('items'))
        .run();
      expect(resp.results.all.records.length).toBeGreaterThanOrEqual(2);
    });

    it('handle: concurrent Promise.all reads resolve correctly (rid-demux)', async () => {
      const app = client!.db(handleDb);
      // Fire overlapping round-trips. The server multiplexes responses by rid;
      // the client readLoop demultiplexes so each promise resolves with its own
      // response regardless of completion order. Each must return its OWN row.
      const [h1, h2, missing] = await Promise.all([
        app.query('items').where(filter.eq('id', 'H1')).rows(),
        app.query('items').where(filter.eq('id', 'H2')).rows(),
        app.query('items').where(filter.eq('id', 'NOPE')).rows(),
      ]);
      expect(h1.length).toBe(1);
      expect(h1[0].id).toBe('H1');
      expect(h2.length).toBe(1);
      expect(h2[0].id).toBe('H2');
      expect(missing.length).toBe(0);
    });

    it('handle: db.dropTable(main, items) drops via HMAC', async () => {
      const app = client!.db(handleDb);
      const qr = await app.dropTable('main', 'items');
      const row = qr.records[0] as Record<string, unknown>;
      expect(row.dropped_table).toBe('items');
      expect(row.existed).toBe(true);
    });

    // ── 13. db.tx() auto-managed transactions (TS-T16) ──────────────────

    let txWrapDb: string;

    it('db.tx: setup acct table', async () => {
      txWrapDb = await setupDb(client!, 'tx_wrap', ['acct']);
    });

    it('db.tx: insert committed; row visible after commit', async () => {
      const app = client!.db(txWrapDb);
      const receipt = await app.tx(async (t) => {
        await t.run(write.insert('acct', [{ id: 'a', bal: 100 }]));
      });

      expect(receipt).toMatchObject({ status: 'committed', state: 'visible' });
      expect((await app.query('acct').rows()).map((r) => r.id)).toContain('a');
    });

    it('db.tx: rollback on throw; row not visible', async () => {
      const app = client!.db(txWrapDb);
      await expect(
        app.tx(async (t) => {
          await t.run(write.insert('acct', [{ id: 'ghost' }]));
          throw new Error('x');
        }),
      ).rejects.toThrow('x');

      const rows = await app.query('acct').rows();
      expect(rows.map((r) => r.id)).not.toContain('ghost');
    });

    // ── 14. Nested batches (P6 live-server e2e) ─────────────────────────

    let nestedDb: string;

    it('nested: setup users + orders + inventory + log tables', async () => {
      nestedDb = await setupDb(client!, 'nested', ['users', 'orders', 'inventory', 'log']);
      // seed one user and two orders we'll reference
      await seed(client!, nestedDb, 'users', [{ id: 'u-alice', name: 'Alice' }]);
      await seed(client!, nestedDb, 'orders', [
        { id: 'o1', user_id: 'u-alice', item: 'widget', qty: 1 },
        { id: 'o2', user_id: 'u-alice', item: 'gear', qty: 2 },
        { id: 'o3', user_id: 'u-bob', item: 'bolt', qty: 10 },
      ]);
    });

    it('nested: happy path — bind + $param + outer reads sub result', async () => {
      const db = client!.db(nestedDb);

      // inner batch: read orders using $param for user_id (param resolution in WHERE is supported)
      const innerBatch = Batch.create('inner-read')
        .add('found', db.query('orders').where(filter.eq('user_id', filter.param('uid'))));

      const resp = await db
        .batch('nested-happy')
        .add('user', db.query('users').where(filter.eq('id', 'u-alice')))
        .subBatch('proc', innerBatch, {
          bind: { uid: filter.queryRef('@user', '[0].id') },
        })
        .add('conf', db.query('orders').where(
          filter.eq('user_id', filter.queryRef('@proc', '.found.records[0].user_id'))
        ))
        .run();

      // proc sub-batch must have succeeded and found alice's orders
      const procResult = resp.results.proc as unknown as Record<string, unknown>;
      expect(procResult).toBeDefined();

      // conf should find alice's orders too (same user_id resolved via @proc)
      const confRecords = resp.results.conf.records;
      expect(confRecords.length).toBeGreaterThanOrEqual(1);
      expect(confRecords[0].user_id).toBe('u-alice');

      // execution_plan: user in stage 0, proc in stage 1 (after user), conf in stage 2+ (after proc)
      const plan = resp.execution_plan;
      expect(Array.isArray(plan)).toBe(true);
      const userStage = plan.findIndex((stage: string[]) => stage.includes('user'));
      const procStage = plan.findIndex((stage: string[]) => stage.includes('proc'));
      const confStage = plan.findIndex((stage: string[]) => stage.includes('conf'));
      expect(userStage).toBeGreaterThanOrEqual(0);
      expect(procStage).toBeGreaterThan(userStage);
      expect(confStage).toBeGreaterThan(procStage);
    });

    it('nested: P3b — $param in INSERT column value (canonical sub-batch write)', async () => {
      const db = client!.db(nestedDb);

      // Inner batch: INSERT a log entry whose ref_user column comes from $param uid.
      // The row value uses the raw { $param: 'uid' } wire shape — P3b feature.
      // We cast to unknown→Record to work around the strict WireValue type (the server
      // accepts $param objects in insert values starting from this commit).
      const innerInsert = Batch.create('inner-insert').add(
        'ins',
        write.insert('log', [
          { event: 'order_created', ref_user: { $param: 'uid' } as unknown as string },
        ]),
      );

      const resp = await db
        .batch('nested-p3b')
        .add('user', db.query('users').where(filter.eq('id', 'u-alice')))
        .subBatch('proc', innerInsert, {
          bind: { uid: filter.queryRef('@user', '[0].id') },
        })
        .run();

      // The sub-batch must have executed without error.
      expect(resp.results.proc).toBeDefined();

      // Read back the log entry and confirm ref_user == 'u-alice'.
      const rows = await db
        .query('log')
        .where(filter.eq('event', 'order_created'))
        .rows();
      const inserted = rows.find((r: Record<string, unknown>) => r.ref_user === 'u-alice');
      expect(inserted).toBeDefined();
      expect(inserted!.ref_user).toBe('u-alice');
    });

    it('nested: atomicity — failed inner op rolls back first inner write', async () => {
      const db = client!.db(nestedDb);

      // inner batch: first write to inventory succeeds, second write to
      // a non-existent table triggers an error — the whole sub-batch must roll back
      const innerAtomic = Batch.create('inner-atomic')
        .add('good', write.insert('inventory', [{ id: 'inv-ghost', sku: 'X1', qty: 5 }]))
        .add('bad', write.insert('no_such_table', [{ id: 'x' }]), { after: ['good'] })
        .transactional();

      // The outer batch itself is non-transactional; we just expect an error
      // propagation from the sub-batch
      await expect(
        db.batch('nested-atomic')
          .subBatch('proc', innerAtomic, {})
          .run(),
      ).rejects.toThrow();

      // The good write must not have persisted (sub-batch rolled back)
      const rows = await db.query('inventory').where(filter.eq('id', 'inv-ghost')).rows();
      expect(rows.length).toBe(0);
    });

    it('nested: tx-in-tx rejected — outer transactional + transactional subBatch', async () => {
      const db = client!.db(nestedDb);

      const innerTx = Batch.create('inner-tx')
        .add('ins', write.insert('log', [{ event: 'test' }]))
        .transactional();

      // The outer transactional batch aborts because the sub-batch is also transactional.
      // The server returns a BatchResponse with transaction.status = "aborted" and
      // a reason containing "nested_tx_not_supported". The client resolves (not rejects).
      const resp = await db
        .batch('nested-tx-in-tx')
        .add('pre', write.insert('log', [{ event: 'outer' }]))
        .subBatch('sub', innerTx, {})
        .transactional()
        .run();

      expect(resp.transaction).toBeDefined();
      expect(resp.transaction!.status).toBe('aborted');
      expect(resp.transaction!.reason).toMatch(/nested_tx_not_supported/i);
    });
  },
);
