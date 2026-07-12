/**
 * End-to-end DDL lifecycle tests — every DDL op from builders/ddl.ts
 * exercised against a live shamir-server.
 *
 * Own startServer (ephemeral port) — no conflict with e2e.test.ts.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, Query, ddl, write } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
  setupDb,
  seed,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

/**
 * Minimal valid WASM module — `(module)` in WAT.
 * 8 bytes: magic + version. Base64-encoded for the wire `wasm` field.
 */
const EMPTY_WASM_B64 = 'AGFzbQEAAAA=';

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e DDL lifecycle (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-ddl] connection failed. Server logs:\n' + server.logs());
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

    // ── 1. Database lifecycle ──────────────────────────────────────────

    it('db: createDb -> listDatabases contains -> dropDb -> list does not contain', async () => {
      const name = uniqueDbName('ddl_db');

      // create
      br(await Batch.create('mk-db')
        .add('c', ddl.createDb(name))
        .execute(client!, 'default'));

      // list — must contain
      const ls1 = br(await Batch.create('ls-db-1')
        .add('l', ddl.listDatabases())
        .execute(client!, 'default'));
      const dbs1 = ls1.results.l.records[0].databases as string[];
      expect(dbs1).toContain(name);

      // drop (HMAC)
      br(await Batch.create('drop-db')
        .add('d', ddl.dropDb(client!, name, { cascade: true }))
        .execute(client!, 'default'));

      // list — must NOT contain
      const ls2 = br(await Batch.create('ls-db-2')
        .add('l', ddl.listDatabases())
        .execute(client!, 'default'));
      const dbs2 = ls2.results.l.records[0].databases as string[];
      expect(dbs2).not.toContain(name);
    });

    it('db: createDb with if_not_exists does not error on duplicate', async () => {
      const name = uniqueDbName('ddl_ifne');

      br(await Batch.create('mk1')
        .add('c', ddl.createDb(name))
        .execute(client!, 'default'));

      // second create with if_not_exists — should not throw
      br(await Batch.create('mk2')
        .add('c', ddl.createDb(name, { if_not_exists: true }))
        .execute(client!, 'default'));

      // cleanup
      br(await Batch.create('cleanup')
        .add('d', ddl.dropDb(client!, name, { cascade: true }))
        .execute(client!, 'default'));
    });

    // ── 2. Repo lifecycle ──────────────────────────────────────────────

    it('repo: createRepo -> listRepos contains -> dropRepo -> list does not contain', async () => {
      const db = uniqueDbName('ddl_repo');
      br(await Batch.create('mk-db')
        .add('c', ddl.createDb(db))
        .execute(client!, 'default'));

      const repoName = 'test_repo';

      // create repo
      br(await Batch.create('mk-repo')
        .add('r', ddl.createRepo(repoName))
        .execute(client!, db));

      // list repos — must contain
      const ls1 = br(await Batch.create('ls-repo-1')
        .add('l', ddl.listRepos())
        .execute(client!, db));
      const repos1 = ls1.results.l.records[0].repos as string[];
      expect(repos1).toContain(repoName);

      // drop repo (HMAC)
      br(await Batch.create('drop-repo')
        .add('d', ddl.dropRepo(client!, db, repoName, { cascade: true }))
        .execute(client!, db));

      // list repos — must NOT contain
      const ls2 = br(await Batch.create('ls-repo-2')
        .add('l', ddl.listRepos())
        .execute(client!, db));
      const repos2 = ls2.results.l.records[0].repos as string[];
      expect(repos2).not.toContain(repoName);

      // cleanup
      br(await Batch.create('cleanup-db')
        .add('d', ddl.dropDb(client!, db, { cascade: true }))
        .execute(client!, 'default'));
    });

    // ── 3. Table lifecycle ─────────────────────────────────────────────

    it('table: createTable -> listTables contains -> dropTable -> list does not contain', async () => {
      const db = uniqueDbName('ddl_tbl');
      br(await Batch.create('mk-db')
        .add('c', ddl.createDb(db))
        .execute(client!, 'default'));
      br(await Batch.create('mk-repo')
        .add('r', ddl.createRepo('main'))
        .execute(client!, db));

      const tbl = 'widgets';

      // create table
      br(await Batch.create('mk-tbl')
        .add('t', ddl.createTable(tbl, { repo: 'main' }))
        .execute(client!, db));

      // list tables — must contain
      const ls1 = br(await Batch.create('ls-tbl-1')
        .add('l', ddl.listTables({ repo: 'main' }))
        .execute(client!, db));
      const tbls1 = ls1.results.l.records[0].tables as string[];
      expect(tbls1).toContain(tbl);

      // drop table (HMAC)
      br(await Batch.create('drop-tbl')
        .add('d', ddl.dropTable(client!, db, 'main', tbl))
        .execute(client!, db));

      // list tables — must NOT contain
      const ls2 = br(await Batch.create('ls-tbl-2')
        .add('l', ddl.listTables({ repo: 'main' }))
        .execute(client!, db));
      const tbls2 = ls2.results.l.records[0].tables as string[];
      expect(tbls2).not.toContain(tbl);

      // cleanup
      br(await Batch.create('cleanup')
        .add('d', ddl.dropDb(client!, db, { cascade: true }))
        .execute(client!, 'default'));
    });

    it('table: createTable with if_not_exists does not error on duplicate', async () => {
      const db = await setupDb(client!, 'ddl_tbl_ifne', ['items']);

      // second create with if_not_exists — should not throw
      br(await Batch.create('mk2')
        .add('t', ddl.createTable('items', { repo: 'main', if_not_exists: true }))
        .execute(client!, db));
    });

    // ── 4. Index lifecycle ─────────────────────────────────────────────

    it('index: createIndex -> listIndexes contains -> dropIndex -> list does not contain', async () => {
      const db = await setupDb(client!, 'ddl_idx', ['products']);

      // create index (hash, not sorted — matches the pattern in e2e.test.ts)
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('by_sku', 'products', [['sku']]))
        .execute(client!, db));

      // list indexes — must contain
      const ls1 = br(await Batch.create('ls-idx-1')
        .add('l', ddl.listIndexes('products'))
        .execute(client!, db));
      const idxNames1 = (ls1.results.l.records[0].indexes as Array<{ name: string }>)
        .map(i => i.name);
      expect(idxNames1).toContain('by_sku');

      // drop index (HMAC)
      br(await Batch.create('drop-idx')
        .add('d', ddl.dropIndex(client!, db, 'main', 'products', 'by_sku'))
        .execute(client!, db));

      // list indexes — must NOT contain
      const ls2 = br(await Batch.create('ls-idx-2')
        .add('l', ddl.listIndexes('products'))
        .execute(client!, db));
      const idxNames2 = (ls2.results.l.records[0].indexes as Array<{ name: string }>)
        .map(i => i.name);
      expect(idxNames2).not.toContain('by_sku');
    });

    // ── 5. Function + function folder lifecycle ────────────────────────

    it('function: createFunctionFolder + createFunction (wasm) -> listFunctions -> renameFunction -> dropFunction', async () => {
      const db = await setupDb(client!, 'ddl_fn', ['t']);

      // create folder
      br(await Batch.create('mk-folder')
        .add('f', ddl.createFunctionFolder(['utils']))
        .execute(client!, db));

      // list folders — verify no error
      const lsf = br(await Batch.create('ls-folders')
        .add('l', ddl.listFunctionFolders())
        .execute(client!, db));
      expect(lsf.results.l.records).toBeDefined();

      // create function from minimal WASM (avoids cargo-compile dependency)
      br(await Batch.create('mk-fn')
        .add('f', ddl.createFunction('my_fn', {
          wasm: EMPTY_WASM_B64,
        }))
        .execute(client!, db));

      // list functions — must contain
      const ls1 = br(await Batch.create('ls-fn-1')
        .add('l', ddl.listFunctions())
        .execute(client!, db));
      const fnList1 = JSON.stringify(ls1.results.l.records);
      expect(fnList1).toContain('my_fn');

      // rename
      br(await Batch.create('ren-fn')
        .add('r', ddl.renameFunction('my_fn', 'my_fn_v2'))
        .execute(client!, db));

      // list functions — renamed
      const ls2 = br(await Batch.create('ls-fn-2')
        .add('l', ddl.listFunctions())
        .execute(client!, db));
      const fnList2 = JSON.stringify(ls2.results.l.records);
      expect(fnList2).toContain('my_fn_v2');

      // drop
      br(await Batch.create('drop-fn')
        .add('d', ddl.dropFunction('my_fn_v2'))
        .execute(client!, db));

      // list functions — must NOT contain
      const ls3 = br(await Batch.create('ls-fn-3')
        .add('l', ddl.listFunctions())
        .execute(client!, db));
      const fnList3 = JSON.stringify(ls3.results.l.records);
      expect(fnList3).not.toContain('my_fn_v2');
    });

    // ── 6. Validator lifecycle (imperative) ────────────────────────────

    it('validator: create (wasm) -> bind -> listValidators -> unbind -> drop', async () => {
      const db = await setupDb(client!, 'ddl_val', ['orders']);

      // create validator from minimal WASM
      br(await Batch.create('mk-val')
        .add('v', ddl.createValidator('check_qty', {
          wasm: EMPTY_WASM_B64,
        }))
        .execute(client!, db));

      // list global validators — must contain
      const lsGlobal = br(await Batch.create('ls-val-global')
        .add('l', ddl.listValidators_())
        .execute(client!, db));
      const valList = JSON.stringify(lsGlobal.results.l.records);
      expect(valList).toContain('check_qty');

      // bind to table
      br(await Batch.create('bind-val')
        .add('b', ddl.bindValidator('check_qty', 'orders', ['insert', 'update'], 1000, {
          db,
          repo: 'main',
        }))
        .execute(client!, db));

      // listValidators for table — must show a binding
      const lsBound = br(await Batch.create('ls-val-bound')
        .add('l', ddl.listValidators('orders', { db, repo: 'main' }))
        .execute(client!, db));
      const boundRow = lsBound.results.l.records[0] as Record<string, unknown>;
      const boundValidators = boundRow.validators as Array<Record<string, unknown>>;
      expect(boundValidators.length).toBeGreaterThan(0);
      expect(boundValidators[0].priority).toBe(1000);

      // unbind
      br(await Batch.create('unbind-val')
        .add('u', ddl.unbindValidator('check_qty', {
          db,
          repo: 'main',
          table: 'orders',
        }))
        .execute(client!, db));

      // listValidators for table — must show no bindings
      const lsUnbound = br(await Batch.create('ls-val-unbound')
        .add('l', ddl.listValidators('orders', { db, repo: 'main' }))
        .execute(client!, db));
      const unboundRow = lsUnbound.results.l.records[0] as Record<string, unknown>;
      const unboundValidators = unboundRow.validators as Array<Record<string, unknown>>;
      expect(unboundValidators.length).toBe(0);

      // drop validator
      br(await Batch.create('drop-val')
        .add('d', ddl.dropValidator('check_qty'))
        .execute(client!, db));

      // list global validators — must NOT contain
      const lsAfterDrop = br(await Batch.create('ls-val-after')
        .add('l', ddl.listValidators_())
        .execute(client!, db));
      const afterDropList = JSON.stringify(lsAfterDrop.results.l.records);
      expect(afterDropList).not.toContain('check_qty');
    });

    // ── 7. Buffer config lifecycle ─────────────────────────────────────

    it('buffer-config: setBufferConfig -> getBufferConfig reflects -> alterBufferConfig', async () => {
      const db = await setupDb(client!, 'ddl_buf', ['data']);

      // set buffer config (all required fields per BufferConfigDto)
      br(await Batch.create('set-buf')
        .add('s', ddl.setBufferConfig('data', {
          max_bytes: 1048576,
          max_entries: 500,
          flush_interval_ms: 2000,
          flush_batch_size: 100,
        }))
        .execute(client!, db));

      // get buffer config — must reflect
      const get1 = br(await Batch.create('get-buf-1')
        .add('g', ddl.getBufferConfig('data'))
        .execute(client!, db));
      const row1 = get1.results.g.records[0] as Record<string, unknown>;
      const cfg1 = row1.config as Record<string, unknown>;
      expect(cfg1.max_entries).toBe(500);
      expect(cfg1.flush_interval_ms).toBe(2000);

      // alter buffer config (partial update)
      br(await Batch.create('alter-buf')
        .add('a', ddl.alterBufferConfig('data', {
          max_entries: 1000,
        }))
        .execute(client!, db));

      // get buffer config — must reflect alter
      const get2 = br(await Batch.create('get-buf-2')
        .add('g', ddl.getBufferConfig('data'))
        .execute(client!, db));
      const row2 = get2.results.g.records[0] as Record<string, unknown>;
      const cfg2 = row2.config as Record<string, unknown>;
      expect(cfg2.max_entries).toBe(1000);
    });

    // ── 8. Retention lifecycle ─────────────────────────────────────────

    it('retention: setRetention -> insert data -> changesSince -> purgeHistory', async () => {
      const db = await setupDb(client!, 'ddl_ret', ['events']);

      // set retention — keep up to 100 versions
      br(await Batch.create('set-ret')
        .add('r', ddl.setRetention(client!, db, 'events', { max_count: 100 }))
        .execute(client!, db));

      // seed some data to create versions
      await seed(client!, db, 'events', [
        { id: 'e1', kind: 'click' },
        { id: 'e2', kind: 'view' },
      ]);

      // changesSince from version 0
      const cs = br(await Batch.create('changes')
        .add('c', ddl.changesSince(0))
        .execute(client!, db));
      expect(cs.results.c.records).toBeDefined();

      // purgeHistory — purge everything older than 0 seconds
      br(await Batch.create('purge')
        .add('p', ddl.purgeHistory(client!, db, 'events', ddl.olderThanAge(0)))
        .execute(client!, db));

      // No error means purge succeeded — the round-trip is the test.
    });

    // ── 9. Migrations lifecycle ────────────────────────────────────────

    it('migration: startMigration -> migrationStatus -> rollbackMigration', async () => {
      const db = await setupDb(client!, 'ddl_mig', ['migdata']);

      // Seed some data
      await seed(client!, db, 'migdata', [
        { id: 'm1', val: 'hello' },
      ]);

      // Create a second repo with in_memory engine to migrate to
      br(await Batch.create('mk-dst-repo')
        .add('r', ddl.createRepo('dst_repo', { engine: 'in_memory' }))
        .execute(client!, db));

      // start migration (HMAC) — use in_memory engine (the only supported one)
      const startResp = br(await Batch.create('start-mig')
        .add('m', ddl.startMigration(
          client!,
          db,
          'main',
          'migdata',
          'dst_repo',
          'in_memory',
        ))
        .execute(client!, db));
      const migRow = startResp.results.m.records[0] as Record<string, unknown>;
      const migId = migRow.migration_id as string;
      expect(typeof migId).toBe('string');
      expect(migId.length).toBeGreaterThan(0);

      // migration status
      const statusResp = br(await Batch.create('mig-status')
        .add('s', ddl.migrationStatus(migId))
        .execute(client!, db));
      const statusRow = statusResp.results.s.records[0] as Record<string, unknown>;
      expect(statusRow).toBeDefined();

      // rollback the migration (HMAC)
      br(await Batch.create('roll-mig')
        .add('r', ddl.rollbackMigration(client!, db, migId))
        .execute(client!, db));
    });

    // G.3 (C3): commit-path of a migration — start (cutover_ready) →
    // commit (committed) → dst repo readable with all seed rows → status
    // after commit is not_found (active map entry removed by the server).
    it('migration: startMigration -> commitMigration -> dst readable -> status not_found', async () => {
      const db = await setupDb(client!, 'ddl_migc', ['migdata']);

      // Seed known rows into the source table.
      await seed(client!, db, 'migdata', [
        { id: 'c1', val: 'alpha' },
        { id: 'c2', val: 'bravo' },
        { id: 'c3', val: 'charlie' },
      ]);

      // Destination repo (in_memory is the only supported engine).
      br(await Batch.create('mk-dst-repo-c')
        .add('r', ddl.createRepo('dst_repo_c', { engine: 'in_memory' }))
        .execute(client!, db));

      // start migration (HMAC) — server runs snapshot+drain+cutover_ready
      // synchronously, so phase is "cutover_ready" on return.
      const startResp = br(await Batch.create('start-mig-c')
        .add('m', ddl.startMigration(
          client!,
          db,
          'main',
          'migdata',
          'dst_repo_c',
          'in_memory',
        ))
        .execute(client!, db));
      const migRow = startResp.results.m.records[0] as Record<string, unknown>;
      expect(migRow.phase).toBe('cutover_ready');
      const migId = migRow.migration_id as string;
      expect(typeof migId).toBe('string');
      expect(migId.length).toBeGreaterThan(0);

      // commit migration (HMAC) — final drain, bulk-populate index2,
      // remove from active map.
      const commitResp = br(await Batch.create('commit-mig')
        .add('c', ddl.commitMigration(client!, db, migId))
        .execute(client!, db));
      const commitRow = commitResp.results.c.records[0] as Record<string, unknown>;
      expect(commitRow.phase).toBe('committed');
      expect(commitRow.migration_id).toBe(migId);
      // dst_records reflects the 3 seed rows copied over.
      const dstRecords = commitRow.dst_records as number;
      expect(dstRecords).toBe(3);

      // Read the migrated table from the dst repo — all seed rows present.
      const readResp = br(await Batch.create('read-dst-c')
        .add('r', Query.withRepo('dst_repo_c', 'migdata'))
        .execute(client!, db));
      const rows = readResp.results.r.records as Array<Record<string, unknown>>;
      const ids = rows.map(r => r.id).sort();
      expect(ids).toEqual(['c1', 'c2', 'c3']);
      // spot-check one value to prove the payload survived the cutover.
      const c2 = rows.find(r => r.id === 'c2');
      expect(c2?.val).toBe('bravo');

      // Status after commit — the server removed the migration from its
      // active map, so a status query on the (now-terminal) id fails with
      // code "not_found".
      try {
        const statusResp = br(await Batch.create('mig-status-after-commit')
          .add('s', ddl.migrationStatus(migId))
          .execute(client!, db));
        // If the server returned an empty record set instead of erroring,
        // treat that as the not_found signal.
        const statusRecs = statusResp.results.s.records;
        expect(statusRecs.length).toBe(0);
      } catch (e: unknown) {
        const msg = (e as Error).message;
        expect(msg).toMatch(/not_found/);
      }
    });

    // ── 10. Schema DDL basic round-trip (boundary for #210) ────────────

    it('schema: setTableSchema -> getTableSchema -> addSchemaRule -> removeSchemaRule', async () => {
      const db = await setupDb(client!, 'ddl_sch', ['users']);

      // set table schema
      br(await Batch.create('set-schema')
        .add('s', ddl.setTableSchema('users', [
          ddl.field(['name']).string().required().build(),
          ddl.field(['age']).int().min(0).max(200).build(),
        ]))
        .execute(client!, db));

      // get table schema — must reflect
      const get1 = br(await Batch.create('get-schema-1')
        .add('g', ddl.getTableSchema('users'))
        .execute(client!, db));
      const schema1 = get1.results.g.records[0] as Record<string, unknown>;
      expect(schema1).toBeDefined();
      const rules1 = JSON.stringify(schema1);
      expect(rules1).toContain('name');
      expect(rules1).toContain('age');

      // add schema rule
      br(await Batch.create('add-rule')
        .add('a', ddl.addSchemaRule('users',
          ddl.field(['email']).string().required().build(),
        ))
        .execute(client!, db));

      // get table schema — must contain email
      const get2 = br(await Batch.create('get-schema-2')
        .add('g', ddl.getTableSchema('users'))
        .execute(client!, db));
      const rules2 = JSON.stringify(get2.results.g.records[0]);
      expect(rules2).toContain('email');

      // remove schema rule
      br(await Batch.create('rm-rule')
        .add('r', ddl.removeSchemaRule('users', ['email']))
        .execute(client!, db));

      // get table schema — email gone
      const get3 = br(await Batch.create('get-schema-3')
        .add('g', ddl.getTableSchema('users'))
        .execute(client!, db));
      const rules3 = JSON.stringify(get3.results.g.records[0]);
      expect(rules3).not.toContain('email');
    });

    // ── 11. List admin ops ─────────────────────────────────────────────

    it('admin: listUsers returns without error', async () => {
      const resp = br(await Batch.create('ls-users')
        .add('l', ddl.listUsers())
        .execute(client!, 'default'));
      expect(resp.results.l.records).toBeDefined();
    });
  },
);

describe('e2e-ddl.test skip reason', () => {
  it('reports why the DDL e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-ddl] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
