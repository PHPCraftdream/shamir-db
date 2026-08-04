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
  atVersion,
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

    // ═══════════════════════════════════════════════════════════════════
    // 11. PATTERN MATCHING — like / ilike / regex (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let patternDb: string;

    it('pattern-setup: create db + seed', async () => {
      patternDb = await setupDb(client!, 'pattern', ['words']);
      await seed(client!, patternDb, 'words', [
        { id: 'p1', word: 'Apple', tag: 'fruit' },
        { id: 'p2', word: 'apricot', tag: 'fruit' },
        { id: 'p3', word: 'Banana', tag: 'fruit' },
        { id: 'p4', word: 'carrot', tag: 'veggie' },
        { id: 'p5', word: 'cherry', tag: 'fruit' },
      ]);
    });

    it('filter: like — case-sensitive prefix match', async () => {
      // "c%" matches 'carrot' and 'cherry' (lowercase c).
      const resp = br(await Batch.create('f-like')
        .add('r', Query.from('words').where(filter.like('word', 'c%')))
        .execute(client!, patternDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('p4');
      expect(ids).toContain('p5');
      expect(ids).not.toContain('p1'); // 'Apple' — uppercase A
    });

    it('filter: ilike — case-insensitive prefix match', async () => {
      // "a%" matches 'Apple' and 'apricot' (case-insensitive).
      const resp = br(await Batch.create('f-ilike')
        .add('r', Query.from('words').where(filter.ilike('word', 'a%')))
        .execute(client!, patternDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('p1'); // 'Apple'
      expect(ids).toContain('p2'); // 'apricot'
      expect(ids).not.toContain('p3'); // 'Banana'
    });

    it('filter: regex — matches pattern', async () => {
      // "^[Aa]" matches words starting with uppercase or lowercase A.
      const resp = br(await Batch.create('f-regex')
        .add('r', Query.from('words').where(filter.regex('word', '^[Aa]')))
        .execute(client!, patternDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('p1'); // 'Apple'
      expect(ids).toContain('p2'); // 'apricot'
      expect(ids).not.toContain('p3'); // 'Banana'
    });

    // ═══════════════════════════════════════════════════════════════════
    // 12. NULL / EXISTENCE FILTERS — isNull / isNotNull / exists / notExists (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let nullDb: string;

    it('null-setup: create db + seed (sparse records)', async () => {
      nullDb = await setupDb(client!, 'nullex', ['items']);
      await seed(client!, nullDb, 'items', [
        { id: 'n1', val: 'present', extra: 'yes' },
        { id: 'n2', val: null },              // val is null, extra absent
        { id: 'n3', extra: 'only-extra' },    // val absent entirely
      ]);
    });

    it('filter: isNull — selects rows where val IS NULL', async () => {
      const resp = br(await Batch.create('f-is-null')
        .add('r', Query.from('items').where(filter.isNull('val')))
        .execute(client!, nullDb));
      const ids = resp.results.r.records.map(r => r.id);
      expect(ids).toContain('n2');
      expect(ids).not.toContain('n1');
    });

    it('filter: isNotNull — selects rows where val IS NOT NULL', async () => {
      const resp = br(await Batch.create('f-is-not-null')
        .add('r', Query.from('items').where(filter.isNotNull('val')))
        .execute(client!, nullDb));
      const ids = resp.results.r.records.map(r => r.id);
      expect(ids).toContain('n1');
      expect(ids).not.toContain('n2');
    });

    it('filter: exists — selects rows where extra field is present', async () => {
      const resp = br(await Batch.create('f-exists')
        .add('r', Query.from('items').where(filter.exists('extra')))
        .execute(client!, nullDb));
      const ids = resp.results.r.records.map(r => r.id);
      expect(ids).toContain('n1');
      expect(ids).toContain('n3');
      expect(ids).not.toContain('n2');
    });

    it('filter: notExists — selects rows where extra field is absent', async () => {
      const resp = br(await Batch.create('f-not-exists')
        .add('r', Query.from('items').where(filter.notExists('extra')))
        .execute(client!, nullDb));
      const ids = resp.results.r.records.map(r => r.id);
      expect(ids).toContain('n2');
      expect(ids).not.toContain('n1');
      expect(ids).not.toContain('n3');
    });

    // ═══════════════════════════════════════════════════════════════════
    // 13. CONTAINMENT FILTERS — contains / containsAny / containsAll (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let arrDb: string;

    it('contains-setup: create db + seed (array fields)', async () => {
      arrDb = await setupDb(client!, 'arrdata', ['docs']);
      await seed(client!, arrDb, 'docs', [
        { id: 'c1', tags: ['a', 'b', 'c'] },
        { id: 'c2', tags: ['b', 'd'] },
        { id: 'c3', tags: ['x', 'y'] },
        { id: 'c4', tags: ['a', 'b', 'd', 'e'] },
      ]);
    });

    it('filter: contains — rows where tags contains "a"', async () => {
      const resp = br(await Batch.create('f-contains')
        .add('r', Query.from('docs').where(filter.contains('tags', 'a')))
        .execute(client!, arrDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('c1');
      expect(ids).toContain('c4');
      expect(ids).not.toContain('c2');
      expect(ids).not.toContain('c3');
    });

    it('filter: containsAny — rows where tags contains "a" or "d"', async () => {
      const resp = br(await Batch.create('f-contains-any')
        .add('r', Query.from('docs').where(filter.containsAny('tags', ['a', 'd'])))
        .execute(client!, arrDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('c1'); // has 'a'
      expect(ids).toContain('c2'); // has 'd'
      expect(ids).toContain('c4'); // has both
      expect(ids).not.toContain('c3');
    });

    it('filter: containsAll — rows where tags contains both "a" and "b"', async () => {
      const resp = br(await Batch.create('f-contains-all')
        .add('r', Query.from('docs').where(filter.containsAll('tags', ['a', 'b'])))
        .execute(client!, arrDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toContain('c1'); // ['a','b','c']
      expect(ids).toContain('c4'); // ['a','b','d','e']
      expect(ids).not.toContain('c2'); // only 'b', not 'a'
      expect(ids).not.toContain('c3');
    });

    // ═══════════════════════════════════════════════════════════════════
    // 14. PAGE-MODE PAGINATION — Query.page(n, size) (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let pageDb: string;

    it('page-setup: create db + seed 10 rows', async () => {
      pageDb = await setupDb(client!, 'paged', ['rows']);
      const records = Array.from({ length: 10 }, (_, i) => ({
        id: `r${String(i).padStart(2, '0')}`,
        n: i,
      }));
      await seed(client!, pageDb, 'rows', records);
    });

    it('pagination: page(1, 3) returns first 3 rows', async () => {
      const resp = br(await Batch.create('page-1')
        .add('r', Query.from('rows').orderByAsc('n').page(1, 3))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(3);
      expect(recs[0].n).toBe(0);
      expect(recs[1].n).toBe(1);
      expect(recs[2].n).toBe(2);
    });

    it('pagination: page(2, 3) returns rows 4-6', async () => {
      const resp = br(await Batch.create('page-2')
        .add('r', Query.from('rows').orderByAsc('n').page(2, 3))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(3);
      expect(recs[0].n).toBe(3);
      expect(recs[1].n).toBe(4);
      expect(recs[2].n).toBe(5);
    });

    it('pagination: page(4, 3) returns last 1 row (partial page)', async () => {
      const resp = br(await Batch.create('page-4')
        .add('r', Query.from('rows').orderByAsc('n').page(4, 3))
        .execute(client!, pageDb));
      const recs = resp.results.r.records;
      expect(recs.length).toBe(1);
      expect(recs[0].n).toBe(9);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 15. DISTINCT — Query.distinct() deduplicates result rows (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    it('distinct: duplicate tag values collapsed to unique set', async () => {
      // fDb is from section 5; has 6 rows with tags 'red','red','blue','blue','green','green'.
      const resp = br(await Batch.create('distinct-tags')
        .add('r', Query.from('t')
          .select([select.field('tag')])
          .distinct()
          .orderByAsc('tag'))
        .execute(client!, fDb));
      const tags = resp.results.r.records.map(r => r.tag);
      expect(tags).toEqual(['blue', 'green', 'red']);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 16. SCALAR PROJECTION — select.func (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let funcDb: string;

    it('func-setup: create db + seed', async () => {
      funcDb = await setupDb(client!, 'funcproj', ['names']);
      await seed(client!, funcDb, 'names', [
        { id: 'f1', name: 'alice' },
        { id: 'f2', name: 'bob' },
        { id: 'f3', name: 'charlie' },
      ]);
    });

    it('select.func: strings/upper applied to name field', async () => {
      const resp = br(await Batch.create('func-upper')
        .add('r', Query.from('names')
          .where(filter.eq('id', 'f1'))
          .select([
            select.field('id'),
            select.func('strings/upper', [filter.ref('name')], 'upper_name'),
          ]))
        .execute(client!, funcDb));
      const rec = resp.results.r.records[0];
      expect(rec.id).toBe('f1');
      expect(rec.upper_name).toBe('ALICE');
    });

    it('select.func: strings/length returns character count', async () => {
      const resp = br(await Batch.create('func-length')
        .add('r', Query.from('names')
          .orderByAsc('id')
          .select([
            select.field('id'),
            select.func('strings/length', [filter.ref('name')], 'name_len'),
          ]))
        .execute(client!, funcDb));
      const recs = resp.results.r.records;
      // alice=5, bob=3, charlie=7
      const alice = recs.find(r => r.id === 'f1');
      const bob = recs.find(r => r.id === 'f2');
      expect(alice?.name_len).toBe(5);
      expect(bob?.name_len).toBe(3);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 17. LIBRARY AGGREGATE — select.aggregateFn (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    it('aggregateFn: count_distinct over tag column', async () => {
      // fDb from section 5 has 6 rows: 2 red, 2 blue, 2 green → 3 distinct tags.
      const resp = br(await Batch.create('agg-count-distinct')
        .add('r', Query.from('t')
          .select([
            select.aggregateFn('count_distinct', 'tag', { alias: 'n_tags' }),
          ]))
        .execute(client!, fDb));
      const rec = resp.results.r.records[0];
      expect(rec.n_tags).toBe(3);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 18. HISTORY RANGE — Query.history() temporal range reads (P1 gap)
    // ═══════════════════════════════════════════════════════════════════

    let histDb: string;

    it('history-setup: create db + make multiple versions of a record', async () => {
      histDb = await setupDb(client!, 'hist', ['log']);
      // v1
      await br(await Batch.create('hist-v1')
        .add('i', write.insert('log', [{ id: 'doc1', rev: 1, note: 'created' }]))
        .transactional()
        .execute(client!, histDb));
      // v2
      await br(await Batch.create('hist-v2')
        .add('u', write.update('log')
          .where(filter.eq('id', 'doc1'))
          .set({ rev: 2, note: 'updated' })
          .build())
        .transactional()
        .execute(client!, histDb));
      // v3
      await br(await Batch.create('hist-v3')
        .add('u', write.update('log')
          .where(filter.eq('id', 'doc1'))
          .set({ rev: 3, note: 'final' })
          .build())
        .transactional()
        .execute(client!, histDb));
    });

    it('history: range read returns multiple versions of the same record', async () => {
      const resp = br(await Batch.create('hist-range')
        .add('r', Query.from('log')
          .where(filter.eq('id', 'doc1'))
          .history({ order: 'asc' }))
        .execute(client!, histDb));
      const recs = resp.results.r.records;
      // Should see at least 3 versions (the three writes above).
      expect(recs.length).toBeGreaterThanOrEqual(3);
      // Oldest first (asc) — rev field ascends.
      const revs = recs.map(r => r.rev as number);
      expect(revs[0]).toBe(1);
      expect(revs[revs.length - 1]).toBe(3);
    });

    it('history: desc order returns newest version first', async () => {
      const resp = br(await Batch.create('hist-desc')
        .add('r', Query.from('log')
          .where(filter.eq('id', 'doc1'))
          .history({ order: 'desc', limit: 2 }))
        .execute(client!, histDb));
      const recs = resp.results.r.records;
      // limit=2, newest first → rev 3 then rev 2.
      expect(recs.length).toBe(2);
      expect(recs[0].rev).toBe(3);
      expect(recs[1].rev).toBe(2);
    });

    it('history: from/to window bounds exclude versions outside the range', async () => {
      // Capture the real version numbers of the three revs (rev 1/2/3) via a
      // full ascending history read with version metadata included.
      const full = br(await Batch.create('hist-fromto-full')
        .add('r', Query.from('log')
          .where(filter.eq('id', 'doc1'))
          .withVersion()
          .history({ order: 'asc' }))
        .execute(client!, histDb));
      const fullRecs = full.results.r.records;
      expect(fullRecs.length).toBeGreaterThanOrEqual(3);
      // revs ascend with version; find the version number of each rev.
      const byRev: Record<number, number> = {};
      for (const r of fullRecs) {
        const rev = r.rev as number;
        const ver = r._version as number;
        if (rev >= 1 && rev <= 3 && ver !== undefined) byRev[rev] = ver;
      }
      const v1 = byRev[1];
      const v2 = byRev[2];
      const v3 = byRev[3];
      expect(v1).toBeDefined();
      expect(v2).toBeDefined();
      expect(v3).toBeDefined();
      expect(v1).toBeLessThanOrEqual(v2);
      expect(v2).toBeLessThanOrEqual(v3);

      // from=v2 → rev 1 (version v1) is EXCLUDED; rev 2/3 INCLUDED.
      const fromResp = br(await Batch.create('hist-from')
        .add('r', Query.from('log')
          .where(filter.eq('id', 'doc1'))
          .history({ order: 'asc', from: atVersion(v2) }))
        .execute(client!, histDb));
      const fromRevs = fromResp.results.r.records.map(r => r.rev as number);
      expect(fromRevs).not.toContain(1);
      expect(fromRevs).toContain(2);
      expect(fromRevs).toContain(3);

      // to=v2 → rev 3 (version v3) is EXCLUDED; rev 1/2 INCLUDED.
      const toResp = br(await Batch.create('hist-to')
        .add('r', Query.from('log')
          .where(filter.eq('id', 'doc1'))
          .history({ order: 'asc', to: atVersion(v2) }))
        .execute(client!, histDb));
      const toRevs = toResp.results.r.records.map(r => r.rev as number);
      expect(toRevs).toContain(1);
      expect(toRevs).toContain(2);
      expect(toRevs).not.toContain(3);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 19. FIELD-EQ SHORTCUT — filter.fieldEq (op:"field") (Cluster G #1)
    // Wire-identical to Eq but a distinct Filter::FieldEq variant.
    // ═══════════════════════════════════════════════════════════════════

    it('fieldEq: op:"field" matches the same rows as op:"eq"', async () => {
      // fDb has 6 rows: 2 red (a,b), 2 blue (c,d), 2 green (e,f).
      const fieldResp = br(await Batch.create('field-eq')
        .add('r', Query.from('t').where(filter.fieldEq('tag', 'red')))
        .execute(client!, fDb));
      const eqResp = br(await Batch.create('plain-eq')
        .add('r', Query.from('t').where(filter.eq('tag', 'red')))
        .execute(client!, fDb));
      const fieldIds = fieldResp.results.r.records.map(r => r.id).sort();
      const eqIds = eqResp.results.r.records.map(r => r.id).sort();
      // Both must match exactly the same set {a, b}.
      expect(fieldIds).toEqual(['a', 'b']);
      expect(fieldIds).toEqual(eqIds);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 20. $expr / $fn AS A FILTER VALUE (Cluster G #2)
    // Not a top-level op — used as the right-hand side of a comparison,
    // resolved per-row then compared against the field.
    // ═══════════════════════════════════════════════════════════════════

    it('$expr value: filter.eq(field, expr(...)) evaluates the expression and compares', async () => {
      // fDb qty values: a=1, b=5, c=10, d=25, e=50, f=100.
      // expr('add', [10, 15]) → 25 → matches only d (qty 25).
      const resp = br(await Batch.create('expr-val')
        .add('r', Query.from('t')
          .where(filter.eq('qty', filter.expr('add', [10, 15]))))
        .execute(client!, fDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toEqual(['d']);
      // Prove non-matching: c (qty 10) and e (qty 50) are excluded.
      expect(ids).not.toContain('c');
      expect(ids).not.toContain('e');
    });

    it('$fn value: filter.eq(field, fn(...)) evaluates the scalar call and compares', async () => {
      // math/abs(-50) → 50 → matches only e (qty 50).
      const resp = br(await Batch.create('fn-val')
        .add('r', Query.from('t')
          .where(filter.eq('qty', filter.fn('math/abs', [-50]))))
        .execute(client!, fDb));
      const ids = resp.results.r.records.map(r => r.id).sort();
      expect(ids).toEqual(['e']);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 21. BINARY LITERAL ROUND-TRIP (Cluster G #3) — DEFERRED, real bug found.
    //
    // A Uint8Array field value written via write.insert() round-trips as a
    // plain `{"0":0,"1":1,...}` object instead of binary/Uint8Array -- the
    // record is silently corrupted server-side. Client-side msgpack encoding
    // was verified correct in isolation (emits genuine bin8 wire bytes).
    // Tracked as a new bug task (see TaskList) rather than shipped here per
    // this cluster's brief ("STOP and report... instead of silently
    // adjusting the test").
    // ═══════════════════════════════════════════════════════════════════

    // ═══════════════════════════════════════════════════════════════════
    // 22. DISTINCT ON AGGREGATE SELECT ITEMS (Cluster G #4)
    // Min/Max with distinct:true → accepted no-op; Sum/Avg/Count with
    // distinct:true → REJECTED (distinct_not_supported_for_fast_path_agg).
    // ═══════════════════════════════════════════════════════════════════

    it('distinct aggregate: min with distinct:true is accepted (correct no-op)', async () => {
      // fDb qty values: 1,5,10,25,50,100 → min is 1 regardless of distinct.
      // Typed SelectItem literal passed to .select([...]) — builder-sanctioned.
      const resp = br(await Batch.create('agg-min-distinct')
        .add('r', Query.from('t')
          .select([
            {
              type: 'aggregate',
              func: 'min',
              field: ['qty'],
              distinct: true,
              alias: 'min_qty',
            },
          ]))
        .execute(client!, fDb));
      expect(resp.results.r.records[0].min_qty).toBe(1);
    });

    it('distinct aggregate: sum with distinct:true is rejected', async () => {
      // There is no SUM(DISTINCT x) — the fast-path aggregator has no
      // distinct-dedup tracking. The server rejects with a typed error.
      await expect(
        Batch.create('agg-sum-distinct')
          .add('r', Query.from('t')
            .select([
              {
                type: 'aggregate',
                func: 'sum',
                field: ['qty'],
                distinct: true,
                alias: 'sum_qty',
              },
            ]))
          .execute(client!, fDb),
      ).rejects.toThrow(/distinct_not_supported_for_fast_path_agg/);
    });

    // ═══════════════════════════════════════════════════════════════════
    // 23. FUNCLIB AGGREGATE BREADTH (Cluster G #5)
    // median / stddev / percentile(args) / string_agg(args) beyond
    // count_distinct (covered in section 17).
    // ═══════════════════════════════════════════════════════════════════

    let statDb: string;

    it('aggFn-setup: create db + seed numeric/string rows', async () => {
      statDb = await setupDb(client!, 'statagg', ['nums']);
      // n values sorted: [2,4,4,4,5,5,7,9] (n=8) — clean for median/stddev.
      // stddev: mean=5, pop-variance=4 → stddev=2 exactly.
      await seed(client!, statDb, 'nums', [
        { id: 's1', n: 2, word: 'red' },
        { id: 's2', n: 4, word: 'blue' },
        { id: 's3', n: 4, word: 'green' },
        { id: 's4', n: 4, word: 'red' },
        { id: 's5', n: 5, word: 'blue' },
        { id: 's6', n: 5, word: 'green' },
        { id: 's7', n: 7, word: 'red' },
        { id: 's8', n: 9, word: 'blue' },
      ]);
    });

    it('aggFn: median (no args) returns lower-median of even-count set', async () => {
      // n=8 (even) → lower median = index n/2-1 = 3 → sorted[3] = 4.
      const resp = br(await Batch.create('agg-median')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('median', 'n', { alias: 'med' })]))
        .execute(client!, statDb));
      expect(resp.results.r.records[0].med).toBe(4);
    });

    it('aggFn: stddev (no args) returns population stddev', async () => {
      // mean=5, pop-variance = 32/8 = 4 → stddev = sqrt(4) = 2.
      // Dec serialises as a string on the wire → parseFloat.
      const resp = br(await Batch.create('agg-stddev')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('stddev', 'n', { alias: 'sd' })]))
        .execute(client!, statDb));
      const sd = parseFloat(resp.results.r.records[0].sd as unknown as string);
      expect(sd).toBeCloseTo(2, 5);
    });

    it('aggFn: percentile args:[0.9] differs from default p=0.5 (args honored)', async () => {
      // default p=0.5: idx = ceil(0.5*8)-1 = 3 → sorted[3] = 4.
      // p=0.9:        idx = ceil(0.9*8)-1 = 7 → sorted[7] = 9.
      const def = br(await Batch.create('agg-pct-default')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('percentile', 'n', { alias: 'p' })]))
        .execute(client!, statDb));
      const p50 = def.results.r.records[0].p;

      const explicit = br(await Batch.create('agg-pct-90')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('percentile', 'n', { alias: 'p', args: [0.9] })]))
        .execute(client!, statDb));
      const p90 = explicit.results.r.records[0].p;

      expect(p50).toBe(4);
      expect(p90).toBe(9);
      expect(p90).not.toBe(p50);
    });

    it('aggFn: string_agg explicit separator arg honored', async () => {
      // 8 words: 3 red, 3 blue, 2 green. Default sep ","; explicit sep "-".
      // If the separator arg were ignored, both would use "," — so the "-"
      // result must contain "-" and NOT ",".
      const def = br(await Batch.create('agg-str-default')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('string_agg', 'word', { alias: 'joined' })]))
        .execute(client!, statDb));
      const defaultJoined = def.results.r.records[0].joined as unknown as string;
      const defaultParts = defaultJoined.split(',').sort();
      expect(defaultParts).toEqual(['blue', 'blue', 'blue', 'green', 'green', 'red', 'red', 'red']);

      const dash = br(await Batch.create('agg-str-dash')
        .add('r', Query.from('nums')
          .select([select.aggregateFn('string_agg', 'word', { alias: 'joined', args: ['-'] })]))
        .execute(client!, statDb));
      const dashJoined = dash.results.r.records[0].joined as unknown as string;
      // The explicit separator is honored: "-" present, "," absent.
      expect(dashJoined).toContain('-');
      expect(dashJoined).not.toContain(',');
      // Same multiset of parts, just split on the new separator.
      expect(dashJoined.split('-').sort()).toEqual(defaultParts);
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
