/**
 * Single-record CRUD via the Batch API — Insert / Set (upsert) /
 * Update / Delete / Read against a fresh isolated database.
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;

  test('setup: create db + repo + table', async () => {
    db = await fixtures.setupDb(client, 'crud', ['items']);
    assert(db, 'db name returned');
  });

  test('insert single record', async () => {
    const resp = await client.execute(db, {
      id: 'ins-one',
      queries: {
        ins: {
          insert_into: 'items',
          values: [{ id: 'A1', name: 'widget', qty: 10 }],
        },
      },
    });
    const inserted = resp.results.ins.records;
    assertEq(inserted.length, 1);
  });

  test('read all returns the inserted record', async () => {
    const resp = await client.execute(db, {
      id: 'read-all',
      queries: { all: { from: 'items' } },
    });
    const recs = resp.results.all.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].id, 'A1');
    assertEq(recs[0].qty, 10);
  });

  test('set (upsert) a new key', async () => {
    await client.execute(db, {
      id: 'set-new',
      queries: {
        s: { set: 'items', key: { id: 'B2' }, value: { id: 'B2', name: 'gear', qty: 3 } },
      },
    });
    const resp = await client.execute(db, {
      id: 'count-after-set',
      queries: { all: { from: 'items' } },
    });
    assertEq(resp.results.all.records.length, 2);
  });

  test('set (upsert) overwrites an existing key', async () => {
    await client.execute(db, {
      id: 'set-existing',
      queries: {
        s: { set: 'items', key: { id: 'A1' }, value: { id: 'A1', name: 'widget-v2', qty: 99 } },
      },
    });
    const resp = await client.execute(db, {
      id: 'read-A1',
      queries: {
        a: { from: 'items', where: { op: 'eq', field: ['id'], value: 'A1' } },
      },
    });
    assertEq(resp.results.a.records.length, 1);
    assertEq(resp.results.a.records[0].name, 'widget-v2');
    assertEq(resp.results.a.records[0].qty, 99);
  });

  test('update by filter', async () => {
    await client.execute(db, {
      id: 'upd',
      queries: {
        u: {
          update: 'items',
          where: { op: 'eq', field: ['id'], value: 'B2' },
          set: { qty: 7 },
        },
      },
    });
    const resp = await client.execute(db, {
      id: 'read-B2',
      queries: {
        b: { from: 'items', where: { op: 'eq', field: ['id'], value: 'B2' } },
      },
    });
    assertEq(resp.results.b.records[0].qty, 7);
  });

  test('delete by filter', async () => {
    await client.execute(db, {
      id: 'del',
      queries: {
        d: {
          delete_from: 'items',
          where: { op: 'eq', field: ['id'], value: 'A1' },
        },
      },
    });
    const resp = await client.execute(db, {
      id: 'read-after-del',
      queries: { all: { from: 'items' } },
    });
    assertEq(resp.results.all.records.length, 1);
    assertEq(resp.results.all.records[0].id, 'B2');
  });

  test('delete remaining and read empty', async () => {
    await client.execute(db, {
      id: 'del-all',
      queries: {
        d: {
          delete_from: 'items',
          where: { op: 'eq', field: ['id'], value: 'B2' },
        },
      },
    });
    const resp = await client.execute(db, {
      id: 'read-empty',
      queries: { all: { from: 'items' } },
    });
    assertEq(resp.results.all.records.length, 0);
  });
};
