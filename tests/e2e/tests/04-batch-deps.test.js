/**
 * Cross-query dependencies via `{"$query": "@alias", "path": "[0].field"}`.
 *
 * `@` is the explicit reference marker (per spec) and is mandatory in
 * the `$query` value — distinguishes a reference from a literal string.
 * Stripped on the server before lookup against the queries map (whose
 * keys never carry `@`).
 *
 * `path` syntax:
 *   "[N].field"   — Nth record's field    (scalar substitution)
 *   "[].field"    — all records' field    (column → IN list expansion)
 */

'use strict';

const { Query, filter } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;

  test('setup: users + orders', async () => {
    db = await fixtures.setupDb(client, 'deps', ['users', 'orders']);
    await fixtures.seed(client, db, 'users', [
      { id: 'u1', name: 'Alice', email: 'alice@x' },
      { id: 'u2', name: 'Bob', email: 'bob@x' },
    ]);
    await fixtures.seed(client, db, 'orders', [
      { id: 'o1', user_id: 'u1', total: 100 },
      { id: 'o2', user_id: 'u2', total: 50 },
      { id: 'o3', user_id: 'u1', total: 250 },
      { id: 'o4', user_id: 'u1', total: 30 },
    ]);
  });

  test('parent → child via @user[0].id', async () => {
    const resp = await client.execute(db, {
      id: 'parent-child',
      queries: {
        user: Query.from('users').where(filter.eq('name', 'Alice')).build(),
        orders: Query.from('orders')
          .where(filter.eq('user_id', filter.queryRef('@user', '[0].id')))
          .build(),
      },
    });
    const orders = resp.results.orders.records;
    assertEq(orders.length, 3);
    for (const o of orders) assertEq(o.user_id, 'u1');
  });

  test('execution_plan reflects the dep (two stages)', async () => {
    const resp = await client.execute(db, {
      id: 'plan-shape',
      queries: {
        user: Query.from('users').where(filter.eq('id', 'u1')).build(),
        orders: Query.from('orders')
          .where(filter.eq('user_id', filter.queryRef('@user', '[0].id')))
          .build(),
      },
    });
    const plan = resp.execution_plan;
    assertEq(plan.length, 2, `expected 2 stages, got ${JSON.stringify(plan)}`);
    assertEq(plan[0][0], 'user');
    assertEq(plan[1][0], 'orders');
  });

  test('column ref via @alias[].field — IN expansion', async () => {
    const resp = await client.execute(db, {
      id: 'array-ref',
      queries: {
        all_users: Query.from('users').build(),
        their_orders: Query.from('orders')
          .where(filter.in_('user_id', [filter.queryRef('@all_users', '[].id')]))
          .build(),
      },
    });
    assertEq(resp.results.their_orders.records.length, 4);
  });

  test('three-step chain: A → B → C', async () => {
    const resp = await client.execute(db, {
      id: 'chain',
      queries: {
        a: Query.from('users').where(filter.eq('id', 'u1')).build(),
        b: Query.from('orders')
          .where(filter.eq('user_id', filter.queryRef('@a', '[0].id')))
          .orderByDesc('total')
          .build(),
        c: Query.from('orders')
          .where(filter.eq('id', filter.queryRef('@b', '[0].id')))
          .build(),
      },
    });
    assertEq(resp.results.c.records.length, 1);
    assertEq(resp.results.c.records[0].total, 250);

    const plan = resp.execution_plan;
    assertEq(plan.length, 3, `expected 3 stages, got ${JSON.stringify(plan)}`);
  });

  test('bare alias (no @) still works as backward-compat', async () => {
    // Documentation prescribes `@user` but the implementation accepts
    // both forms. Pin this so a future strictness change is noticed.
    const resp = await client.execute(db, {
      id: 'bare',
      queries: {
        u: Query.from('users').where(filter.eq('id', 'u2')).build(),
        o: Query.from('orders')
          .where(filter.eq('user_id', filter.queryRef('u', '[0].id')))
          .build(),
      },
    });
    assertEq(resp.results.o.records.length, 1);
    assertEq(resp.results.o.records[0].user_id, 'u2');
  });
};
