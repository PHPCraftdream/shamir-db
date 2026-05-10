/**
 * Multiple INDEPENDENT queries in a single batch — proves the planner
 * groups them in one parallel stage and the response keeps them keyed
 * by alias.
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;

  test('setup: db with three tables', async () => {
    db = await fixtures.setupDb(client, 'multi', ['users', 'orders', 'products']);
    await fixtures.seed(client, db, 'users', [
      { id: 'u1', name: 'Alice' },
      { id: 'u2', name: 'Bob' },
    ]);
    await fixtures.seed(client, db, 'orders', [
      { id: 'o1', user_id: 'u1', total: 100 },
      { id: 'o2', user_id: 'u2', total: 50 },
      { id: 'o3', user_id: 'u1', total: 250 },
    ]);
    await fixtures.seed(client, db, 'products', [
      { id: 'p1', name: 'Widget', price: 9.99 },
      { id: 'p2', name: 'Gear', price: 14.5 },
      { id: 'p3', name: 'Sprocket', price: 22.0 },
      { id: 'p4', name: 'Bolt', price: 0.5 },
    ]);
  });

  test('three independent reads return correct counts', async () => {
    const resp = await client.execute(db, {
      id: 'multi-read',
      queries: {
        u: { from: 'users' },
        o: { from: 'orders' },
        p: { from: 'products' },
      },
    });
    assertEq(Object.keys(resp.results).length, 3);
    assertEq(resp.results.u.records.length, 2);
    assertEq(resp.results.o.records.length, 3);
    assertEq(resp.results.p.records.length, 4);
  });

  test('execution_plan groups independent queries into one stage', async () => {
    const resp = await client.execute(db, {
      id: 'stages',
      queries: {
        u: { from: 'users' },
        o: { from: 'orders' },
        p: { from: 'products' },
      },
    });
    const plan = resp.execution_plan;
    assert(Array.isArray(plan), 'plan must be array');
    // No `$query` deps → single stage with all three aliases.
    assertEq(plan.length, 1, `expected 1 stage, got ${plan.length}: ${JSON.stringify(plan)}`);
    const stage = [...plan[0]].sort();
    assertEq(stage.length, 3);
  });

  test('mixed read + write in one batch', async () => {
    const resp = await client.execute(db, {
      id: 'mixed',
      queries: {
        ins: {
          insert_into: 'products',
          values: [{ id: 'p5', name: 'Nut', price: 0.3 }],
        },
        rd: { from: 'users' },
      },
    });
    assert(resp.results.ins, 'ins result present');
    assert(resp.results.rd, 'rd result present');
    assertEq(resp.results.rd.records.length, 2);
  });

  test('return_all=false shrinks the response', async () => {
    const resp = await client.execute(db, {
      id: 'no-return',
      queries: {
        s: { set: 'users', key: { id: 'u3' }, value: { id: 'u3', name: 'Carol' } },
        keep: { from: 'users' },
      },
      return_all: false,
    });
    // `keep` is a leaf alias the planner kept; `s` may not appear.
    assert(resp.results.keep, 'leaf alias kept');
  });
};
