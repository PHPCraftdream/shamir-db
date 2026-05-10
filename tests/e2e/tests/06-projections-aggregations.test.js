/**
 * SELECT projections + aggregations + GROUP BY.
 *
 * SelectItem schema (from `crates/shamir-query-types/src/read/select.rs`):
 *   { type: 'all' }
 *   { type: 'field',     path: ['user'], alias?: '...' }
 *   { type: 'aggregate', func: 'count'|'sum'|'avg'|'min'|'max',
 *                        field: <AggregateField>, alias?, distinct? }
 *   { type: 'count_all', alias?: '...' }
 *
 * AggregateField for now: `{ path: ['amount'] }` for column aggregates,
 * or omit (use `count_all`) for COUNT(*).
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;

  test('setup: orders', async () => {
    db = await fixtures.setupDb(client, 'agg', ['orders']);
    await fixtures.seed(client, db, 'orders', [
      { id: 'o1', user: 'alice', amount: 100, region: 'eu' },
      { id: 'o2', user: 'alice', amount: 200, region: 'eu' },
      { id: 'o3', user: 'bob', amount: 50, region: 'us' },
      { id: 'o4', user: 'bob', amount: 75, region: 'us' },
      { id: 'o5', user: 'carol', amount: 500, region: 'eu' },
    ]);
  });

  test('select specific fields (column projection)', async () => {
    const resp = await client.execute(db, {
      id: 'proj',
      queries: {
        r: {
          from: 'orders',
          select: {
            items: [
              { type: 'field', path: ['user'] },
              { type: 'field', path: ['amount'] },
            ],
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 5);
    for (const r of recs) {
      assert(!('id' in r), `unexpected id: ${JSON.stringify(r)}`);
      assert(!('region' in r), `unexpected region: ${JSON.stringify(r)}`);
      assert('user' in r);
      assert('amount' in r);
    }
  });

  test('count_all aggregate', async () => {
    const resp = await client.execute(db, {
      id: 'cnt',
      queries: {
        c: {
          from: 'orders',
          select: { items: [{ type: 'count_all', alias: 'n' }] },
        },
      },
    });
    const r = resp.results.c.records;
    assertEq(r.length, 1);
    assertEq(r[0].n, 5);
  });

  test('sum + avg + min + max', async () => {
    const resp = await client.execute(db, {
      id: 'sums',
      queries: {
        s: {
          from: 'orders',
          select: {
            items: [
              { type: 'aggregate', func: 'sum', field: ['amount'], alias: 'total' },
              { type: 'aggregate', func: 'avg', field: ['amount'], alias: 'mean' },
              { type: 'aggregate', func: 'min', field: ['amount'], alias: 'lo' },
              { type: 'aggregate', func: 'max', field: ['amount'], alias: 'hi' },
            ],
          },
        },
      },
    });
    const r = resp.results.s.records[0];
    assertEq(r.total, 925);
    assertEq(r.mean, 185);
    assertEq(r.lo, 50);
    assertEq(r.hi, 500);
  });

  test('group_by user → count + sum', async () => {
    const resp = await client.execute(db, {
      id: 'gb',
      queries: {
        g: {
          from: 'orders',
          group_by: { fields: [['user']] },
          select: {
            items: [
              { type: 'field', path: ['user'] },
              { type: 'count_all', alias: 'n_orders' },
              { type: 'aggregate', func: 'sum', field: ['amount'], alias: 'total' },
            ],
          },
          order_by: { items: [{ field: ['user'], direction: 'asc' }] },
        },
      },
    });
    const recs = resp.results.g.records;
    assertEq(recs.length, 3);
    assertEq(recs[0].user, 'alice');
    assertEq(recs[0].n_orders, 2);
    assertEq(recs[0].total, 300);
    assertEq(recs[1].user, 'bob');
    assertEq(recs[1].total, 125);
    assertEq(recs[2].user, 'carol');
    assertEq(recs[2].total, 500);
  });

  test('group_by region', async () => {
    const resp = await client.execute(db, {
      id: 'gbr',
      queries: {
        g: {
          from: 'orders',
          group_by: { fields: [['region']] },
          select: {
            items: [
              { type: 'field', path: ['region'] },
              { type: 'aggregate', func: 'sum', field: ['amount'], alias: 't' },
            ],
          },
        },
      },
    });
    const byRegion = {};
    for (const r of resp.results.g.records) byRegion[r.region] = r.t;
    assertEq(byRegion.eu, 800);
    assertEq(byRegion.us, 125);
  });
};
