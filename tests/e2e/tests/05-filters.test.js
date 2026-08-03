/**
 * Filter operators — every WHERE op supported by the engine.
 */

'use strict';

const { Query, filter } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assertEq, assert }) {
  let db;
  const seedRecords = [
    { id: 'a', qty: 1, tag: 'red', addr: { city: 'NYC' } },
    { id: 'b', qty: 5, tag: 'red', addr: { city: 'LA' } },
    { id: 'c', qty: 10, tag: 'blue', addr: { city: 'NYC' } },
    { id: 'd', qty: 25, tag: 'blue', addr: { city: 'SF' } },
    { id: 'e', qty: 50, tag: 'green', addr: { city: 'LA' } },
  ];

  test('setup', async () => {
    db = await fixtures.setupDb(client, 'filters', ['t']);
    await fixtures.seed(client, db, 't', seedRecords);
  });

  async function read(where) {
    const resp = await client.execute(db, {
      id: 'r',
      queries: { r: Query.from('t').where(where).build() },
    });
    return resp.results.r.records;
  }

  test('eq', async () => {
    const r = await read(filter.eq('tag', 'red'));
    assertEq(r.length, 2);
  });

  test('ne (neq)', async () => {
    const r = await read(filter.ne('tag', 'red'));
    assertEq(r.length, 3);
  });

  test('gt', async () => {
    const r = await read(filter.gt('qty', 10));
    assertEq(r.length, 2); // 25, 50
  });

  test('gte', async () => {
    const r = await read(filter.gte('qty', 10));
    assertEq(r.length, 3); // 10, 25, 50
  });

  test('lt', async () => {
    const r = await read(filter.lt('qty', 10));
    assertEq(r.length, 2); // 1, 5
  });

  test('lte', async () => {
    const r = await read(filter.lte('qty', 10));
    assertEq(r.length, 3); // 1, 5, 10
  });

  test('in', async () => {
    const r = await read(filter.in_('tag', ['red', 'green']));
    assertEq(r.length, 3);
  });

  test('not_in', async () => {
    const r = await read(filter.notIn('tag', ['red', 'green']));
    assertEq(r.length, 2);
  });

  test('between (from/to, inclusive)', async () => {
    const r = await read(filter.between('qty', 5, 25));
    assertEq(r.length, 3); // 5, 10, 25
  });

  test('and', async () => {
    const r = await read(
      filter.and([filter.eq('tag', 'blue'), filter.gt('qty', 10)]),
    );
    assertEq(r.length, 1);
    assertEq(r[0].id, 'd');
  });

  test('or', async () => {
    const r = await read(
      filter.or([filter.eq('tag', 'green'), filter.gt('qty', 20)]),
    );
    assertEq(r.length, 2); // d (qty=25 blue) + e (qty=50 green)
  });

  test('not', async () => {
    const r = await read(filter.not(filter.eq('tag', 'red')));
    assertEq(r.length, 3);
  });

  test('nested AND/OR', async () => {
    const r = await read(
      filter.and([
        filter.or([filter.eq('tag', 'red'), filter.eq('tag', 'blue')]),
        filter.gte('qty', 5),
      ]),
    );
    assertEq(r.length, 3); // b, c, d
  });

  test('nested field path', async () => {
    const r = await read(filter.eq(['addr', 'city'], 'NYC'));
    assertEq(r.length, 2);
    const ids = r.map((x) => x.id).sort();
    assert(ids.includes('a'));
    assert(ids.includes('c'));
  });
};
