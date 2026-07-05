/**
 * Multiple databases — single session can address any db; no leaks.
 */

'use strict';

const hmac = require('../helpers/hmac');

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
    // The db `dbA` still owns its `main` repo — a plain `drop_db` is now
    // rejected with `still_referenced` (referential-integrity guard added in
    // the replication campaign; see `admin_db_repo.rs::handle_drop_db` and
    // `ddl_wire_e2e/error_codes.rs::error_code_still_referenced_drop_db`).
    // Cascade the drop so the repo+table are removed recursively first.
    await client.execute('default', {
      id: 'rm-a',
      queries: { d: hmac.drop_db_op(client, dbA, { cascade: true }) },
    });
    const resp = await client.execute(dbB, {
      id: 'b-still',
      queries: { all: { from: 't' } },
    });
    assertEq(resp.results.all.records.length, 1);
  });

  test('drop_db without cascade on a db with repos → still_referenced', async () => {
    // New referential-integrity contract (replication campaign): dropping a
    // db that still owns repositories without `cascade: true` is rejected
    // with code `still_referenced`. Pin it here so the contract is covered
    // on the Node side alongside the Rust suite.
    const victim = await fixtures.setupDb(client, 'iso_ref', ['t']);
    let err = null;
    try {
      await client.execute('default', {
        id: 'rm-ref',
        queries: { d: hmac.drop_db_op(client, victim) },
      });
    } catch (e) {
      err = e;
    }
    assert(err, 'expected drop_db without cascade to fail with still_referenced');
    assert(
      /still_referenced/.test(err.message || ''),
      `expected still_referenced, got: ${err.message}`
    );
  });
};
