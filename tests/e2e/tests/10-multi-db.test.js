/**
 * Multiple databases — single session can address any db; no leaks.
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assertEq, assert }) {
  let dbA;
  let dbB;

  test('create two isolated databases', async () => {
    dbA = await fixtures.setupDb(client, 'iso_a', ['t']);
    dbB = await fixtures.setupDb(client, 'iso_b', ['t']);
    assert(dbA !== dbB);
  });

  test('write to A only', async () => {
    await fixtures.seed(client, dbA, 't', [
      { id: 'x1', src: 'A' },
      { id: 'x2', src: 'A' },
    ]);
  });

  test('write to B only', async () => {
    await fixtures.seed(client, dbB, 't', [
      { id: 'y1', src: 'B' },
    ]);
  });

  test('A sees only A records', async () => {
    const resp = await client.execute(dbA, {
      id: 'a-read',
      queries: { all: { from: 't' } },
    });
    const recs = resp.results.all.records;
    assertEq(recs.length, 2);
    for (const r of recs) assertEq(r.src, 'A');
  });

  test('B sees only B records', async () => {
    const resp = await client.execute(dbB, {
      id: 'b-read',
      queries: { all: { from: 't' } },
    });
    const recs = resp.results.all.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].src, 'B');
  });

  test('drop A leaves B intact', async () => {
    await client.execute('default', {
      id: 'rm-a',
      queries: { d: { drop_db: dbA } },
    });
    const resp = await client.execute(dbB, {
      id: 'b-still',
      queries: { all: { from: 't' } },
    });
    assertEq(resp.results.all.records.length, 1);
  });
};
