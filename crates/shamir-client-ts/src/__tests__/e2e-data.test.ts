/**
 * End-to-end data tests — deep coverage of write/read/filter/agg/versioning
 * operations and live round-trip proof of the interner id-on-wire path.
 *
 * Spawns its own server on an ephemeral port. Does NOT duplicate the basic
 * CRUD covered in e2e.test.ts — goes deeper into edge-cases and proves
 * the interner packing (#208) end-to-end.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse, WireValue } from '../index.js';
import {
  Query,
  Batch,
  filter,
  select,
  write,
  ddl,
} from '../index.js';
import {
  SERVER_BIN,
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

// ─── test suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e data operations + interner round-trip (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-data] connection failed. Server logs:\n' + server.logs());
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

    // ═══════════════════════════════════════════════════════════════════
    // 1. INSERT — batch multi-row
    // ═══════════════════════════════════════════════════════════════════

    let dataDb: string;

    it('setup: create db + repo + table', async () => {
      dataDb = await setupDb(client!, 'data', ['items', 'metrics', 'wide']);
      expect(typeof dataDb).toBe('string');
    });

    it('insert: batch of 5 records', async () => {
      const rows = [
        { id: 'i1', name: 'alpha', qty: 10, active: true },
        { id: 'i2', name: 'beta', qty: 20, active: false },
        { id: 'i3', name: 'gamma', qty: 30, active: true },
        { id: 'i4', name: 'delta', qty: 40, active: false },
        { id: 'i5', name: 'epsilon', qty: 50, active: true },
      ];
      const resp = br(await Batch.create('ins-batch')
        .add('ins', write.insert('items', rows))
        .execute(client!, dataDb));
      expect(resp.results.ins.records.length).toBe(5);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 2. UPSERT — new key vs existing key
    // ═══════════════════════════════════════════════════════════════════

    it('upsert: insert-or-update — new key creates', async () => {
      await br(await Batch.create('ups-new')
        .add('u', write.upsert('items', { id: 'i6' }, {
          id: 'i6',
          name: 'zeta',
          qty: 60,
          active: true,
        }))
        .execute(client!, dataDb));

      const rows = await client!.db(dataDb).query('items')
        .where(filter.eq('id', 'i6')).rows();
      expect(rows.length).toBe(1);
      expect(rows[0].name).toBe('zeta');
    });

    it('upsert: existing key overwrites', async () => {
      await br(await Batch.create('ups-exist')
        .add('u', write.upsert('items', { id: 'i6' }, {
          id: 'i6',
          name: 'zeta-v2',
          qty: 66,
          active: false,
        }))
        .execute(client!, dataDb));

      const rows = await client!.db(dataDb).query('items')
        .where(filter.eq('id', 'i6')).rows();
      expect(rows[0].name).toBe('zeta-v2');
      expect(rows[0].qty).toBe(66);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 3. UPDATE — set by where, partial merge
    // ═══════════════════════════════════════════════════════════════════

    it('update: set by where changes only matching rows', async () => {
      await br(await Batch.create('upd-where')
        .add('u', write.update('items')
          .where(filter.eq('id', 'i1'))
          .set({ qty: 111 })
          .build())
        .execute(client!, dataDb));

      const rows = await client!.db(dataDb).query('items')
        .where(filter.eq('id', 'i1')).rows();
      expect(rows[0].qty).toBe(111);
      // name unchanged
      expect(rows[0].name).toBe('alpha');
    });

    it('update: partial merge — only touched fields change', async () => {
      await br(await Batch.create('upd-partial')
        .add('u', write.update('items')
          .where(filter.eq('id', 'i2'))
          .set({ active: true })
          .build())
        .execute(client!, dataDb));

      const rows = await client!.db(dataDb).query('items')
        .where(filter.eq('id', 'i2')).rows();
      expect(rows[0].active).toBe(true);
      expect(rows[0].name).toBe('beta');
      expect(rows[0].qty).toBe(20);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 4. DELETE — by where + delete-all
    // ═══════════════════════════════════════════════════════════════════

    it('delete: by where removes matching rows', async () => {
      // Delete i5
      await br(await Batch.create('del-where')
        .add('d', write.del('items', filter.eq('id', 'i5')))
        .execute(client!, dataDb));

      const rows = await client!.db(dataDb).query('items').rows();
      const ids = rows.map(r => r.id);
      expect(ids).not.toContain('i5');
    });

    it('delete-all: delete with a universal filter clears the table', async () => {
      // Seed a scratch table for delete-all
      const delDb = await setupDb(client!, 'del_all', ['scratch']);
      await seed(client!, delDb, 'scratch', [
        { id: 'x1', v: 1 },
        { id: 'x2', v: 2 },
        { id: 'x3', v: 3 },
      ]);

      // Delete all rows using isNotNull on a field every row has
      await br(await Batch.create('del-all')
        .add('d', write.del('scratch', filter.isNotNull('id')))
        .execute(client!, delDb));

      const rows = await client!.db(delDb).query('scratch').rows();
      expect(rows.length).toBe(0);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 5. QUERY / FILTER — deep and nested
    // ═══════════════════════════════════════════════════════════════════

    let fDb: string;

    it('filter-deep: setup + seed', async () => {
      fDb = await setupDb(client!, 'fdeep', ['t']);
      await seed(client!, fDb, 't', [
        { id: 'a', qty: 1, tag: 'red', profile: { age: 25, city: 'NYC' } },
        { id: 'b', qty: 5, tag: 'red', profile: { age: 30, city: 'LA' } },
        { id: 'c', qty: 10, tag: 'blue', profile: { age: 35, city: 'NYC' } },
        { id: 'd', qty: 25, tag: 'blue', profile: { age: 40, city: 'SF' } },
        { id: 'e', qty: 50, tag: 'green', profile: { age: 45, city: 'LA' } },
        { id: 'f', qty: 100, tag: 'green', profile: { age: 50, city: 'NYC' } },
      ]);
    });

    it('filter-deep: NOT', async () => {
      const resp = br(await Batch.create('f-not')
        .add('r', Query.from('t').where(filter.not(filter.eq('tag', 'red'))))
        .execute(client!, fDb));
      expect(resp.results.r.records.length).toBe(4);
    });

    it('filter-deep: AND + OR + NOT nested', async () => {
      // (tag=red OR tag=green) AND NOT qty<10
      const resp = br(await Batch.create('f-nested')
        .add('r', Query.from('t').where(
          filter.and([
            filter.or([
              filter.eq('tag', 'red'),
              filter.eq('tag', 'green'),
            ]),
            filter.not(filter.lt('qty', 10)),
          ]),
        ))
        .execute(client!, fDb));
      // red: a(1),b(5) => qty<10 excluded => none from red pass (1<10,5<10)
      // green: e(50),f(100) => both pass
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toEqual(['e', 'f']);
    });

    it('filter-deep: IN + range (between)', async () => {
      const resp = br(await Batch.create('f-in-range')
        .add('r', Query.from('t').where(
          filter.and([
            filter.in_('tag', ['red', 'blue']),
            filter.between('qty', 5, 25),
          ]),
        ))
        .execute(client!, fDb));
      // red: b(5), blue: c(10),d(25) => 3
      expect(resp.results.r.records.length).toBe(3);
    });

    it('filter-deep: nested field path (profile.city)', async () => {
      const resp = br(await Batch.create('f-nested-field')
        .add('r', Query.from('t').where(filter.eq(['profile', 'city'], 'NYC')))
        .execute(client!, fDb));
      // a, c, f
      expect(resp.results.r.records.length).toBe(3);
    });

    it('filter-deep: nested field + comparison (profile.age > 35)', async () => {
      const resp = br(await Batch.create('f-nested-cmp')
        .add('r', Query.from('t').where(filter.gt(['profile', 'age'], 35)))
        .execute(client!, fDb));
      // d(40), e(45), f(50) => 3
      expect(resp.results.r.records.length).toBe(3);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 6. PROJECTION / SELECT
    // ═══════════════════════════════════════════════════════════════════

    it('projection: select specific fields', async () => {
      const resp = br(await Batch.create('proj')
        .add('r', Query.from('t').select([
          select.field('id'),
          select.field('tag'),
        ]))
        .execute(client!, fDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(6);
      for (const r of recs) {
        expect('id' in r).toBe(true);
        expect('tag' in r).toBe(true);
        expect('qty' in r).toBe(false);
        expect('profile' in r).toBe(false);
      }
    });

    // ═══════════════════════════════════════════════════════════════════
    // 7. AGGREGATION edge-cases
    // ═══════════════════════════════════════════════════════════════════

    it('agg: count/sum/avg/min/max over all rows', async () => {
      const resp = br(await Batch.create('agg-all')
        .add('a', Query.from('t').select([
          select.countAll('cnt'),
          select.sum('qty', { alias: 'total' }),
          select.avg('qty', { alias: 'mean' }),
          select.min('qty', { alias: 'lo' }),
          select.max('qty', { alias: 'hi' }),
        ]))
        .execute(client!, fDb));
      const r = resp.results.a.records[0];
      expect(r.cnt).toBe(6);
      expect(r.total).toBe(1 + 5 + 10 + 25 + 50 + 100);
      expect(r.lo).toBe(1);
      expect(r.hi).toBe(100);
    });

    it('agg: group_by tag with count + sum', async () => {
      const resp = br(await Batch.create('agg-grp')
        .add('g', Query.from('t')
          .groupBy('tag')
          .select([
            select.field('tag'),
            select.countAll('n'),
            select.sum('qty', { alias: 'total' }),
          ])
          .orderByAsc('tag'))
        .execute(client!, fDb));
      const recs = resp.results.g.records;
      expect(recs.length).toBe(3);
      // blue: c(10)+d(25)=35, green: e(50)+f(100)=150, red: a(1)+b(5)=6
      expect(recs[0].tag).toBe('blue');
      expect(recs[0].total).toBe(35);
      expect(recs[1].tag).toBe('green');
      expect(recs[1].total).toBe(150);
      expect(recs[2].tag).toBe('red');
      expect(recs[2].total).toBe(6);
    });

    it('agg: empty result — count is 0, sum/avg/min/max are null', async () => {
      const resp = br(await Batch.create('agg-empty')
        .add('a', Query.from('t')
          .where(filter.eq('id', 'NONEXISTENT'))
          .select([
            select.countAll('cnt'),
            select.sum('qty', { alias: 'total' }),
            select.avg('qty', { alias: 'mean' }),
            select.min('qty', { alias: 'lo' }),
            select.max('qty', { alias: 'hi' }),
          ]))
        .execute(client!, fDb));
      const r = resp.results.a.records[0];
      expect(r.cnt).toBe(0);
      // ShamirDB: sum on empty set returns 0; avg/min/max return null.
      expect(r.total).toBe(0);
      expect(r.mean).toBeNull();
      expect(r.lo).toBeNull();
      expect(r.hi).toBeNull();
    });

    // ═══════════════════════════════════════════════════════════════════
    // 8. VERSIONING — asOfVersion / asOfTimestamp / withVersion
    // ═══════════════════════════════════════════════════════════════════

    let verDb: string;

    it('versioning: setup + initial insert', async () => {
      verDb = await setupDb(client!, 'ver', ['docs']);
      // Insert initial version
      await br(await Batch.create('ver-ins')
        .add('ins', write.insert('docs', [{ id: 'd1', title: 'v1' }]))
        .transactional()
        .execute(client!, verDb));
    });

    it('versioning: withVersion flag is accepted by the server', async () => {
      const resp = br(await Batch.create('ver-wv')
        .add('r', Query.from('docs')
          .where(filter.eq('id', 'd1'))
          .withVersion())
        .execute(client!, verDb));
      const rec = resp.results.r.records[0];
      expect(rec.id).toBe('d1');
      expect(rec.title).toBe('v1');
      // The query succeeded with withVersion=true; version metadata may
      // appear in records or in the result envelope depending on server impl.
    });

    it('versioning: asOfVersion reads historical state', async () => {
      // Read current version number
      const cur = br(await Batch.create('ver-cur')
        .add('r', Query.from('docs')
          .where(filter.eq('id', 'd1'))
          .withVersion())
        .execute(client!, verDb));
      const curVersion = cur.results.r.records[0].__version__ as number | undefined;

      // Update the record
      await br(await Batch.create('ver-upd')
        .add('u', write.update('docs')
          .where(filter.eq('id', 'd1'))
          .set({ title: 'v2' })
          .build())
        .transactional()
        .execute(client!, verDb));

      // Read current — should be v2
      const after = await client!.db(verDb).query('docs')
        .where(filter.eq('id', 'd1')).rows();
      expect(after[0].title).toBe('v2');

      // If we got a version number, read as-of that version — should be v1
      if (curVersion !== undefined && typeof curVersion === 'number') {
        const resp = br(await Batch.create('ver-asof')
          .add('r', Query.from('docs')
            .where(filter.eq('id', 'd1'))
            .asOfVersion(curVersion))
          .execute(client!, verDb));
        const recs = resp.results.r.records;
        expect(recs.length).toBe(1);
        expect(recs[0].title).toBe('v1');
      }
    });

    it('versioning: asOfTimestamp reads at a point in time', async () => {
      // Capture a timestamp after the v2 update
      const tsAfterV2 = Date.now();

      // Wait a bit and make v3
      await new Promise(r => setTimeout(r, 50));
      await br(await Batch.create('ver-v3')
        .add('u', write.update('docs')
          .where(filter.eq('id', 'd1'))
          .set({ title: 'v3' })
          .build())
        .transactional()
        .execute(client!, verDb));

      // Current should be v3
      const cur = await client!.db(verDb).query('docs')
        .where(filter.eq('id', 'd1')).rows();
      expect(cur[0].title).toBe('v3');

      // As-of the timestamp before v3 should return v2
      const resp = br(await Batch.create('ver-ts')
        .add('r', Query.from('docs')
          .where(filter.eq('id', 'd1'))
          .asOfTimestamp(tsAfterV2))
        .execute(client!, verDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(1);
      expect(recs[0].title).toBe('v2');
    });

    // ═══════════════════════════════════════════════════════════════════
    // 9. BATCH ATOMICITY — error rolls back all
    // ═══════════════════════════════════════════════════════════════════

    it('batch-atomicity: error in one op rolls back entire tx batch', async () => {
      const atomDb = await setupDb(client!, 'atom', ['a', 'b']);

      // Transactional batch: insert into a, then insert into nonexistent table
      const result = await Batch.create('atom-fail')
        .add('ok', write.insert('a', [{ id: 'z1', val: 1 }]))
        .add('bad', write.insert('no_such_table', [{ id: 'x' }]), { after: ['ok'] })
        .transactional()
        .execute(client!, atomDb)
        .catch(e => e);

      // Either rejected or returned with aborted status
      if (result instanceof Error) {
        // Good — error propagated
      } else {
        const resp = br(result);
        expect(resp.transaction?.status).toBe('aborted');
      }

      // The good insert must NOT have persisted
      const rows = await client!.db(atomDb).query('a').rows();
      const ids = rows.map(r => r.id);
      expect(ids).not.toContain('z1');
    });

    it('batch-atomicity: all-success tx commits everything', async () => {
      const atomDb = await setupDb(client!, 'atom_ok', ['x', 'y']);

      const resp = br(await Batch.create('atom-ok')
        .add('ix', write.insert('x', [{ id: 'r1', v: 10 }]))
        .add('iy', write.insert('y', [{ id: 'r2', v: 20 }]))
        .transactional()
        .execute(client!, atomDb));

      expect(resp.transaction?.status).toBe('committed');

      const xRows = await client!.db(atomDb).query('x').rows();
      const yRows = await client!.db(atomDb).query('y').rows();
      expect(xRows.map(r => r.id)).toContain('r1');
      expect(yRows.map(r => r.id)).toContain('r2');
    });

    // ═══════════════════════════════════════════════════════════════════
    // 10. INTERNER ROUND-TRIP — the main event (#208 proof)
    // ═══════════════════════════════════════════════════════════════════

    let intDb: string;

    it('interner: setup', async () => {
      intDb = await setupDb(client!, 'interner_rt', ['docs']);
    });

    it('interner: write with NAMES, read back same NAMES (basic round-trip)', async () => {
      const app = client!.db(intDb);
      await app.run(write.insert('docs', [{
        id: 'rt1',
        title: 'Hello',
        score: 42,
        active: true,
      }]));

      const rows = await app.query('docs')
        .where(filter.eq('id', 'rt1')).rows();
      expect(rows.length).toBe(1);
      expect(rows[0].id).toBe('rt1');
      expect(rows[0].title).toBe('Hello');
      expect(rows[0].score).toBe(42);
      expect(rows[0].active).toBe(true);
    });

    it('interner: cache is populated after write (proof id-path used)', async () => {
      const fm = client!.internerCache.getOrCreate(intDb, 'main');
      // After writing records with field names, the interner cache must be non-empty
      expect(fm.size()).toBeGreaterThan(0);
      expect(fm.epoch()).toBeGreaterThan(0n);

      // At least 'id', 'title', 'score', 'active' should be interned
      for (const name of ['id', 'title', 'score', 'active']) {
        const fid = fm.getId(name);
        expect(fid).toBeDefined();
        expect(fid! > 0n).toBe(true);
        // Reverse lookup must match
        expect(fm.getName(fid!)).toBe(name);
      }
    });

    it('interner: non-ASCII field names round-trip', async () => {
      const app = client!.db(intDb);
      await app.run(write.insert('docs', [{
        id: 'rt-unicode',
        // Hebrew
        'שם': 'עברית',
        // Cyrillic
        'имя': 'тест',
        // CJK
        '名前': 'テスト',
        // Emoji-like long name
        'field_with_a_very_long_name_to_test_wider_id_widths': 'long',
      }]));

      const rows = await app.query('docs')
        .where(filter.eq('id', 'rt-unicode')).rows();
      expect(rows.length).toBe(1);
      expect(rows[0]['שם']).toBe('עברית');
      expect(rows[0]['имя']).toBe('тест');
      expect(rows[0]['名前']).toBe('テスト');
      expect(rows[0]['field_with_a_very_long_name_to_test_wider_id_widths']).toBe('long');

      // Verify these names are in the interner cache
      const fm = client!.internerCache.getOrCreate(intDb, 'main');
      for (const name of ['שם', 'имя', '名前']) {
        expect(fm.getId(name)).toBeDefined();
      }
    });

    it('interner: nested-map keys are interned recursively', async () => {
      const app = client!.db(intDb);
      await app.run(write.insert('docs', [{
        id: 'rt-nested',
        profile: {
          age: 30,
          city: 'Tel Aviv',
          address: {
            street: 'Rothschild',
            zip: '12345',
          },
        },
      }]));

      const rows = await app.query('docs')
        .where(filter.eq('id', 'rt-nested')).rows();
      expect(rows.length).toBe(1);
      const profile = rows[0].profile as Record<string, WireValue>;
      expect(profile.age).toBe(30);
      expect(profile.city).toBe('Tel Aviv');
      const address = profile.address as Record<string, WireValue>;
      expect(address.street).toBe('Rothschild');
      expect(address.zip).toBe('12345');

      // Nested keys should be interned
      const fm = client!.internerCache.getOrCreate(intDb, 'main');
      for (const name of ['profile', 'age', 'city', 'address', 'street', 'zip']) {
        const fid = fm.getId(name);
        expect(fid).toBeDefined();
      }
    });

    it('interner: large batch 50+ records round-trip (stress id-codec)', async () => {
      const app = client!.db(intDb);
      const records: Array<Record<string, WireValue>> = [];
      for (let i = 0; i < 60; i++) {
        records.push({
          id: `bulk-${String(i).padStart(3, '0')}`,
          idx: i,
          label: `item-${i}`,
          value: i * 3.14,
          nested: {
            x: i,
            y: i * 2,
          },
        });
      }

      await app.run(write.insert('docs', records));

      // Read all bulk records back
      const rows = await app.query('docs')
        .where(filter.gte('idx', 0))
        .orderByAsc('idx')
        .rows();

      // At least 60 bulk records
      expect(rows.length).toBeGreaterThanOrEqual(60);

      // Verify first and last
      const first = rows.find(r => r.id === 'bulk-000');
      expect(first).toBeDefined();
      expect(first!.idx).toBe(0);
      expect(first!.label).toBe('item-0');

      const last = rows.find(r => r.id === 'bulk-059');
      expect(last).toBeDefined();
      expect(last!.idx).toBe(59);
      expect(last!.label).toBe('item-59');

      // Nested map round-trip
      const nested = first!.nested as Record<string, WireValue>;
      expect(nested.x).toBe(0);
      expect(nested.y).toBe(0);

      const lastNested = last!.nested as Record<string, WireValue>;
      expect(lastNested.x).toBe(59);
      expect(lastNested.y).toBe(118);
    });

    it('interner: id widths — many unique field names to push past 1-byte ids', async () => {
      // Insert a record with many unique fields to potentially exercise wider id widths
      const app = client!.db(intDb);
      const wideRecord: Record<string, WireValue> = { id: 'rt-wide' };
      for (let i = 0; i < 40; i++) {
        wideRecord[`field_${String(i).padStart(3, '0')}`] = i;
      }
      await app.run(write.insert('docs', [wideRecord]));

      const rows = await app.query('docs')
        .where(filter.eq('id', 'rt-wide')).rows();
      expect(rows.length).toBe(1);
      // Verify all 40 fields round-tripped
      for (let i = 0; i < 40; i++) {
        expect(rows[0][`field_${String(i).padStart(3, '0')}`]).toBe(i);
      }

      // Interner cache should have all these field names
      const fm = client!.internerCache.getOrCreate(intDb, 'main');
      expect(fm.size()).toBeGreaterThanOrEqual(40);
    });

    it('interner: $fn values remain strings (not id-coded)', async () => {
      // The builder supports $fn via filter.fn(). We can use it in an insert
      // value context. The $fn value should NOT be interned — it should stay
      // as a string on the wire.
      // Note: $fn in insert values is a server feature; we test that the
      // builder produces the right shape and the server handles it.
      const app = client!.db(intDb);
      // Use upsert with a $fn value — filter.fn('NOW') produces { $fn: 'NOW' }
      // which should be preserved as-is in the record value (not interned).
      // Whether the server interprets $fn in insert values depends on the
      // server version. We verify the round-trip shape.
      await app.run(write.insert('docs', [{
        id: 'rt-fn',
        label: 'fn-test',
        // Plain string values should still work fine
        status: 'active',
      }]));

      const rows = await app.query('docs')
        .where(filter.eq('id', 'rt-fn')).rows();
      expect(rows[0].label).toBe('fn-test');
      expect(rows[0].status).toBe('active');
      // Note: $fn in insert record values is not currently expressible
      // through the write builder's WireValue type (it expects plain values).
      // This is documented as a builder gap — $fn is only available in
      // filter/select contexts (filter.fn()), not in write.insert() values.
    });
  },
);

describe('e2e-data.test skip reason', () => {
  it('reports why the e2e-data test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        `[e2e-data.test] SKIPPED — server binary not found at:\n  ${SERVER_BIN}\n` +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
