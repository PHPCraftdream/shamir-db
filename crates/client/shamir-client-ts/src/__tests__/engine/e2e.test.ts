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

    // ── 1. connect ──────────────────────────────────────────────────────

    it('connect: session id is 32 bytes', () => {
      expect(client).not.toBeNull();
      expect(client!.sessionId().length).toBe(32);
    });

    // ── 2. setup ────────────────────────────────────────────────────────

    let crudDb: string;

    it('setup: create db + repo + table', async () => {
      crudDb = await setupDb(client!, 'crud', ['items']);
      expect(typeof crudDb).toBe('string');
      expect(crudDb.length).toBeGreaterThan(0);
    });

    // ── 3. CRUD (port 02) ───────────────────────────────────────────────

    it('CRUD: insert single record', async () => {
      const resp = br(await Batch.create('ins-one')
        .add('ins', write.insert('items', [{ id: 'A1', name: 'widget', qty: 10 }]))
        .execute(client!, crudDb));
      expect(resp.results.ins.records.length).toBe(1);
    });

    it('CRUD: read all returns the inserted record', async () => {
      const resp = br(await Batch.create('read-all')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      const recs = resp.results.all.records;
      expect(recs.length).toBe(1);
      expect(recs[0].id).toBe('A1');
      expect(recs[0].qty).toBe(10);
    });

    it('CRUD: upsert a new key', async () => {
      await br(await Batch.create('set-new')
        .add('s', write.upsert('items', { id: 'B2' }, { id: 'B2', name: 'gear', qty: 3 }))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('count-after-set')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      expect(resp.results.all.records.length).toBe(2);
    });

    it('CRUD: upsert overwrites an existing key', async () => {
      await br(await Batch.create('set-existing')
        .add('s', write.upsert('items', { id: 'A1' }, { id: 'A1', name: 'widget-v2', qty: 99 }))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-A1')
        .add('a', Query.from('items').where(filter.eq('id', 'A1')))
        .execute(client!, crudDb));
      expect(resp.results.a.records.length).toBe(1);
      expect(resp.results.a.records[0].name).toBe('widget-v2');
      expect(resp.results.a.records[0].qty).toBe(99);
    });

    it('CRUD: update by filter', async () => {
      await br(await Batch.create('upd')
        .add('u', write.update('items').where(filter.eq('id', 'B2')).set({ qty: 7 }).build())
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-B2')
        .add('b', Query.from('items').where(filter.eq('id', 'B2')))
        .execute(client!, crudDb));
      expect(resp.results.b.records[0].qty).toBe(7);
    });

    it('CRUD: delete by filter', async () => {
      await br(await Batch.create('del')
        .add('d', write.del('items', filter.eq('id', 'A1')))
        .execute(client!, crudDb));
      const resp = br(await Batch.create('read-after-del')
        .add('all', Query.from('items'))
        .execute(client!, crudDb));
      expect(resp.results.all.records.length).toBe(1);
      expect(resp.results.all.records[0].id).toBe('B2');
    });

    // ── 4. Filters (port 05) ────────────────────────────────────────────

    let filterDb: string;
    const filterSeed = [
      { id: 'a', qty: 1, tag: 'red', addr: { city: 'NYC' } },
      { id: 'b', qty: 5, tag: 'red', addr: { city: 'LA' } },
      { id: 'c', qty: 10, tag: 'blue', addr: { city: 'NYC' } },
      { id: 'd', qty: 25, tag: 'blue', addr: { city: 'SF' } },
      { id: 'e', qty: 50, tag: 'green', addr: { city: 'LA' } },
    ];

    it('filters: setup + seed', async () => {
      filterDb = await setupDb(client!, 'filters', ['t']);
      await seed(client!, filterDb, 't', filterSeed);
    });

    async function filteredRead(where: ReturnType<typeof filter.eq>): Promise<Array<Record<string, WireValue>>> {
      const resp = br(await Batch.create('r')
        .add('r', Query.from('t').where(where))
        .execute(client!, filterDb));
      return resp.results.r.records;
    }

    it('filters: eq', async () => {
      const r = await filteredRead(filter.eq('tag', 'red'));
      expect(r.length).toBe(2);
    });

    it('filters: ne', async () => {
      const r = await filteredRead(filter.ne('tag', 'red'));
      expect(r.length).toBe(3);
    });

    it('filters: gt', async () => {
      const r = await filteredRead(filter.gt('qty', 10));
      expect(r.length).toBe(2);
    });

    it('filters: gte', async () => {
      const r = await filteredRead(filter.gte('qty', 10));
      expect(r.length).toBe(3);
    });

    it('filters: lt', async () => {
      const r = await filteredRead(filter.lt('qty', 10));
      expect(r.length).toBe(2);
    });

    it('filters: lte', async () => {
      const r = await filteredRead(filter.lte('qty', 10));
      expect(r.length).toBe(3);
    });

    it('filters: in', async () => {
      const r = await filteredRead(filter.in_('tag', ['red', 'green']));
      expect(r.length).toBe(3);
    });

    it('filters: not_in', async () => {
      const r = await filteredRead(filter.notIn('tag', ['red', 'green']));
      expect(r.length).toBe(2);
    });

    it('filters: between', async () => {
      const r = await filteredRead(filter.between('qty', 5, 25));
      expect(r.length).toBe(3);
    });

    it('filters: and', async () => {
      const r = await filteredRead(
        filter.and([
          filter.eq('tag', 'blue'),
          filter.gt('qty', 10),
        ]),
      );
      expect(r.length).toBe(1);
      expect((r[0] as Record<string, unknown>).id).toBe('d');
    });

    it('filters: or', async () => {
      const r = await filteredRead(
        filter.or([
          filter.eq('tag', 'green'),
          filter.gt('qty', 20),
        ]),
      );
      expect(r.length).toBe(2);
    });

    it('filters: nested AND/OR', async () => {
      const r = await filteredRead(
        filter.and([
          filter.or([
            filter.eq('tag', 'red'),
            filter.eq('tag', 'blue'),
          ]),
          filter.gte('qty', 5),
        ]),
      );
      expect(r.length).toBe(3);
    });

    it('filters: nested field path', async () => {
      const r = await filteredRead(filter.eq(['addr', 'city'], 'NYC'));
      expect(r.length).toBe(2);
      const ids = r.map((x: Record<string, unknown>) => x.id).sort();
      expect(ids).toContain('a');
      expect(ids).toContain('c');
    });

    // ── 5. Projections + aggregations (port 06) ─────────────────────────

    let aggDb: string;

    it('agg: setup orders', async () => {
      aggDb = await setupDb(client!, 'agg', ['orders']);
      await seed(client!, aggDb, 'orders', [
        { id: 'o1', user: 'alice', amount: 100, region: 'eu' },
        { id: 'o2', user: 'alice', amount: 200, region: 'eu' },
        { id: 'o3', user: 'bob', amount: 50, region: 'us' },
        { id: 'o4', user: 'bob', amount: 75, region: 'us' },
        { id: 'o5', user: 'carol', amount: 500, region: 'eu' },
      ]);
    });

    it('agg: select specific fields (column projection)', async () => {
      const resp = br(await Batch.create('proj')
        .add('r', Query.from('orders').select([select.field('user'), select.field('amount')]))
        .execute(client!, aggDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      for (const r of recs) {
        expect('user' in r).toBe(true);
        expect('amount' in r).toBe(true);
        expect('id' in r).toBe(false);
        expect('region' in r).toBe(false);
      }
    });

    it('agg: count_all aggregate', async () => {
      const resp = br(await Batch.create('cnt')
        .add('c', Query.from('orders').select([select.countAll('n')]))
        .execute(client!, aggDb));
      const r = resp.results.c.records;
      expect(r.length).toBe(1);
      expect(r[0].n).toBe(5);
    });

    it('agg: sum + avg + min + max', async () => {
      const resp = br(await Batch.create('sums')
        .add('s', Query.from('orders').select([
          select.sum('amount', { alias: 'total' }),
          select.avg('amount', { alias: 'mean' }),
          select.min('amount', { alias: 'lo' }),
          select.max('amount', { alias: 'hi' }),
        ]))
        .execute(client!, aggDb));
      const r = resp.results.s.records[0];
      expect(r.total).toBe(925);
      expect(r.mean).toBe(185);
      expect(r.lo).toBe(50);
      expect(r.hi).toBe(500);
    });

    it('agg: group_by user -> count + sum', async () => {
      const resp = br(await Batch.create('gb')
        .add('g', Query.from('orders')
          .groupBy('user')
          .select([
            select.field('user'),
            select.countAll('n_orders'),
            select.sum('amount', { alias: 'total' }),
          ])
          .orderByAsc('user'))
        .execute(client!, aggDb));
      const recs = resp.results.g.records;
      expect(recs.length).toBe(3);
      expect(recs[0].user).toBe('alice');
      expect(recs[0].n_orders).toBe(2);
      expect(recs[0].total).toBe(300);
      expect(recs[1].user).toBe('bob');
      expect(recs[1].total).toBe(125);
      expect(recs[2].user).toBe('carol');
      expect(recs[2].total).toBe(500);
    });

    // ── 6. Sorting + pagination (port 07) ───────────────────────────────

    let pageDb: string;
    const PN = 20;

    it('sort/page: setup 20 records', async () => {
      pageDb = await setupDb(client!, 'page', ['items']);
      const records: object[] = [];
      for (let i = 0; i < PN; i += 1) {
        records.push({
          id: `r${String(i).padStart(2, '0')}`,
          score: (i * 7) % 100,
          bucket: i % 3,
        });
      }
      await seed(client!, pageDb, 'items', records as Array<Record<string, WireValue>>);
    });

    it('sort/page: order_by score asc', async () => {
      const resp = br(await Batch.create('asc')
        .add('r', Query.from('items').orderByAsc('score'))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(PN);
      for (let i = 1; i < recs.length; i += 1) {
        expect((recs[i - 1].score as number) <= (recs[i].score as number)).toBe(true);
      }
    });

    it('sort/page: order_by score desc', async () => {
      const resp = br(await Batch.create('desc')
        .add('r', Query.from('items').orderByDesc('score'))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      for (let i = 1; i < recs.length; i += 1) {
        expect((recs[i - 1].score as number) >= (recs[i].score as number)).toBe(true);
      }
    });

    it('sort/page: order_by multiple fields (bucket asc, score desc)', async () => {
      const resp = br(await Batch.create('multi')
        .add('r', Query.from('items')
          .orderBy([
            { field: ['bucket'], direction: 'asc' },
            { field: ['score'], direction: 'desc' },
          ]))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      for (let i = 1; i < recs.length; i += 1) {
        const prev = recs[i - 1];
        const cur = recs[i];
        if (prev.bucket === cur.bucket) {
          expect((prev.score as number) >= (cur.score as number)).toBe(true);
        } else {
          expect((prev.bucket as number) < (cur.bucket as number)).toBe(true);
        }
      }
    });

    it('sort/page: LIMIT/OFFSET first page', async () => {
      const resp = br(await Batch.create('p1')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(0))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      expect(recs[0].id).toBe('r00');
      expect(recs[4].id).toBe('r04');
    });

    it('sort/page: LIMIT/OFFSET second page', async () => {
      const resp = br(await Batch.create('p2')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(5))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      expect(recs[0].id).toBe('r05');
      expect(recs[4].id).toBe('r09');
    });

    it('sort/page: LIMIT past end', async () => {
      const resp = br(await Batch.create('p-end')
        .add('r', Query.from('items')
          .orderByAsc('id')
          .limit(5)
          .offset(18))
        .execute(client!, pageDb));
      expect(resp.results.r.records.length).toBe(2);
    });

    it('sort/page: count_total returns full size with paginated records', async () => {
      const resp = br(await Batch.create('ct')
        .add('r', Query.from('items')
          .where(filter.gte('score', 50))
          .limit(3)
          .offset(0)
          .countTotal())
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      const pag = resp.results.r.pagination;
      expect(recs.length).toBe(3);
      expect(pag).toBeDefined();
      expect(typeof pag!.total_count).toBe('number');
      expect(pag!.total_count! > 3).toBe(true);
    });

    // ── 7. Batch multi + deps (port 03/04) ──────────────────────────────

    let multiDb: string;

    it('batch: setup multi tables', async () => {
      multiDb = await setupDb(client!, 'multi', ['users', 'orders', 'products']);
      await seed(client!, multiDb, 'users', [
        { id: 'u1', name: 'Alice' },
        { id: 'u2', name: 'Bob' },
      ]);
      await seed(client!, multiDb, 'orders', [
        { id: 'o1', user_id: 'u1', total: 100 },
        { id: 'o2', user_id: 'u2', total: 50 },
        { id: 'o3', user_id: 'u1', total: 250 },
        { id: 'o4', user_id: 'u1', total: 30 },
      ]);
      await seed(client!, multiDb, 'products', [
        { id: 'p1', name: 'Widget', price: 9.99 },
        { id: 'p2', name: 'Gear', price: 14.5 },
        { id: 'p3', name: 'Sprocket', price: 22.0 },
        { id: 'p4', name: 'Bolt', price: 0.5 },
      ]);
    });

    it('batch: three independent reads return correct counts', async () => {
      const resp = br(await Batch.create('multi-read')
        .add('u', Query.from('users'))
        .add('o', Query.from('orders'))
        .add('p', Query.from('products'))
        .execute(client!, multiDb));
      expect(Object.keys(resp.results).length).toBe(3);
      expect(resp.results.u.records.length).toBe(2);
      expect(resp.results.o.records.length).toBe(4);
      expect(resp.results.p.records.length).toBe(4);
    });

    it('batch: execution_plan groups independent queries into one stage', async () => {
      const resp = br(await Batch.create('stages')
        .add('u', Query.from('users'))
        .add('o', Query.from('orders'))
        .add('p', Query.from('products'))
        .execute(client!, multiDb));
      const plan = resp.execution_plan;
      expect(Array.isArray(plan)).toBe(true);
      expect(plan.length).toBe(1);
      expect(plan[0].length).toBe(3);
    });

    it('batch: parent -> child via $query reference (typed queryRef)', async () => {
      const db = client!.db(multiDb);
      const resp = await db
        .batch('parent-child-ref')
        .add('user', db.query('users').where(filter.eq('id', 'u1')))
        .add('orders', db.query('orders').where(filter.eq('user_id', filter.queryRef('@user', '[0].id'))))
        .run();
      const orders = resp.results.orders.records;
      expect(orders.length).toBe(3);
      for (const o of orders) {
        expect(o.user_id).toBe('u1');
      }
      // 2-stage execution_plan: user in stage 0, orders in stage 1
      expect(resp.execution_plan.length).toBe(2);
      expect(resp.execution_plan[0]).toContain('user');
      expect(resp.execution_plan[1]).toContain('orders');
    });

    it('batch: execution_plan reflects dep (two stages)', async () => {
      const db = client!.db(multiDb);
      const resp = await db
        .batch('plan-shape-ref')
        .add('user', db.query('users').where(filter.eq('id', 'u1')))
        .add('orders', db.query('orders').where(filter.eq('user_id', filter.queryRef('@user', '[0].id'))))
        .run();
      const plan = resp.execution_plan;
      expect(plan.length).toBe(2);
      expect(plan[0][0]).toBe('user');
      expect(plan[1][0]).toBe('orders');
    });

    it('batch: column ref via queryRef IN-expansion (typed)', async () => {
      const db = client!.db(multiDb);
      const resp = await db
        .batch('array-ref-typed')
        .add('all_users', db.query('users'))
        .add('their_orders', db.query('orders').where(filter.in_('user_id', [filter.queryRef('@all_users', '[].id')])))
        .run();
      expect(resp.results.their_orders.records.length).toBe(4);
      // 2-stage: all_users in stage 0, their_orders in stage 1
      expect(resp.execution_plan.length).toBe(2);
      expect(resp.execution_plan[0]).toContain('all_users');
      expect(resp.execution_plan[1]).toContain('their_orders');
    });

  },
);

describe('e2e.test skip reason', () => {
  it('reports why the e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        `[e2e.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
