/**
 * ORDER BY + LIMIT/OFFSET pagination + count_total.
 */

'use strict';

const { Query, filter } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  let db;
  const N = 20;

  test('setup: 20 records', async () => {
    db = await fixtures.setupDb(client, 'page', ['items']);
    const records = [];
    for (let i = 0; i < N; i += 1) {
      records.push({
        id: `r${String(i).padStart(2, '0')}`,
        score: (i * 7) % 100,
        bucket: i % 3,
      });
    }
    await fixtures.seed(client, db, 'items', records);
  });

  test('order_by score asc', async () => {
    const resp = await client.execute(db, {
      id: 'asc',
      queries: {
        r: Query.from('items').orderByAsc('score').build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, N);
    for (let i = 1; i < recs.length; i += 1) {
      assert(
        recs[i - 1].score <= recs[i].score,
        `ascending broken at ${i}: ${recs[i - 1].score} > ${recs[i].score}`
      );
    }
  });

  test('order_by score desc', async () => {
    const resp = await client.execute(db, {
      id: 'desc',
      queries: {
        r: Query.from('items').orderByDesc('score').build(),
      },
    });
    const recs = resp.results.r.records;
    for (let i = 1; i < recs.length; i += 1) {
      assert(recs[i - 1].score >= recs[i].score, 'descending broken');
    }
  });

  test('order_by multiple fields (bucket asc, score desc)', async () => {
    const resp = await client.execute(db, {
      id: 'multi',
      queries: {
        r: Query.from('items').orderByAsc('bucket').orderByDesc('score').build(),
      },
    });
    const recs = resp.results.r.records;
    for (let i = 1; i < recs.length; i += 1) {
      const prev = recs[i - 1];
      const cur = recs[i];
      if (prev.bucket === cur.bucket) {
        assert(prev.score >= cur.score, `secondary order broken at ${i}`);
      } else {
        assert(prev.bucket < cur.bucket, `primary order broken at ${i}`);
      }
    }
  });

  test('LIMIT/OFFSET pagination — first page', async () => {
    const resp = await client.execute(db, {
      id: 'p1',
      queries: {
        r: Query.from('items').orderByAsc('id').limit(5).offset(0).build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 5);
    assertEq(recs[0].id, 'r00');
    assertEq(recs[4].id, 'r04');
  });

  test('LIMIT/OFFSET pagination — second page', async () => {
    const resp = await client.execute(db, {
      id: 'p2',
      queries: {
        r: Query.from('items').orderByAsc('id').limit(5).offset(5).build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 5);
    assertEq(recs[0].id, 'r05');
    assertEq(recs[4].id, 'r09');
  });

  test('LIMIT past end', async () => {
    const resp = await client.execute(db, {
      id: 'p-end',
      queries: {
        r: Query.from('items').orderByAsc('id').limit(5).offset(18).build(),
      },
    });
    assertEq(resp.results.r.records.length, 2); // r18, r19
  });

  test('count_total returns full size with paginated records', async () => {
    const resp = await client.execute(db, {
      id: 'ct',
      queries: {
        r: Query.from('items')
          .where(filter.gte('score', 50))
          .limit(3)
          .offset(0)
          .countTotal()
          .build(),
      },
    });
    const recs = resp.results.r.records;
    const pag = resp.results.r.pagination;
    assertEq(recs.length, 3);
    assert(pag, 'pagination info present');
    assert(typeof pag.total_count === 'number', `total_count present: ${JSON.stringify(pag)}`);
    assert(pag.total_count > 3, `total_count > limit: ${pag.total_count}`);
  });
};
