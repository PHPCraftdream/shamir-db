/**
 * E2E tests for new index2 types: FTS / Functional / Vector.
 *
 * Wire format (CreateIndexOp extended):
 *   { create_index, table, fields, index_type: "fts"|"functional"|"vector",
 *     fts_tokenizer, fts_language,
 *     functional_op,
 *     vector_dim, vector_metric }
 *
 * Filter ops:
 *   { op: "fts", field, query, mode: "and"|"or" }
 *   { op: "computed", expr_op, field, cmp, value }
 *   { op: "vector_similarity", field, query: [f32...], k }
 */

'use strict';

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  // ─────────────────────────────────────────────────────────────────────
  // FTS
  // ─────────────────────────────────────────────────────────────────────

  test('fts: create index, insert, AND query', async () => {
    const db = await fixtures.setupDb(client, 'fts_and', ['posts']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'body_fts',
          table: 'posts',
          fields: [['body']],
          index_type: 'fts',
          fts_tokenizer: 'whitespace',
        },
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'posts', values: [{ body: 'hello rust world' }] },
        w2: { insert_into: 'posts', values: [{ body: 'rust is great' }] },
        w3: { insert_into: 'posts', values: [{ body: 'hello python' }] },
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'posts',
          where: { op: 'fts', field: ['body'], query: 'hello world', mode: 'and' },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].body, 'hello rust world');
    // FTS uses BM25-ranked index path.
    assertEq(resp.results.r.stats.index_used, 'index2_ranked');
  });

  test('fts: OR mode union', async () => {
    const db = await fixtures.setupDb(client, 'fts_or', ['posts']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'body_fts',
          table: 'posts',
          fields: [['body']],
          index_type: 'fts',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'posts', values: [{ body: 'apple orange' }] },
        w2: { insert_into: 'posts', values: [{ body: 'banana pear' }] },
        w3: { insert_into: 'posts', values: [{ body: 'cherry grape' }] },
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'posts',
          where: { op: 'fts', field: ['body'], query: 'apple banana', mode: 'or' },
        },
      },
    });
    assertEq(resp.results.r.records.length, 2);
  });

  test('fts: case-insensitive tokenization', async () => {
    const db = await fixtures.setupDb(client, 'fts_case', ['posts']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: { create_index: 'b', table: 'posts', fields: [['body']], index_type: 'fts' },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: { w: { insert_into: 'posts', values: [{ body: 'HELLO World' }] } },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'posts',
          where: { op: 'fts', field: ['body'], query: 'hello WORLD', mode: 'and' },
        },
      },
    });
    assertEq(resp.results.r.records.length, 1);
  });

  test('fts: brute-force fallback without index', async () => {
    const db = await fixtures.setupDb(client, 'fts_brute', ['posts']);
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'posts', values: [{ body: 'hello world' }] },
        w2: { insert_into: 'posts', values: [{ body: 'no match' }] },
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'posts',
          where: { op: 'fts', field: ['body'], query: 'hello', mode: 'and' },
        },
      },
    });
    assertEq(resp.results.r.records.length, 1);
  });

  // ─────────────────────────────────────────────────────────────────────
  // Functional
  // ─────────────────────────────────────────────────────────────────────

  test('functional: LOWER(email) = lookup', async () => {
    const db = await fixtures.setupDb(client, 'fn_lower', ['users']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'email_lower',
          table: 'users',
          fields: [['email']],
          index_type: 'functional',
          functional_op: 'lower',
        },
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'users', values: [{ email: 'Alice@FOO.com', name: 'alice' }] },
        w2: { insert_into: 'users', values: [{ email: 'BOB@bar.org', name: 'bob' }] },
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'users',
          where: {
            op: 'computed',
            expr_op: 'lower',
            field: ['email'],
            cmp: 'eq',
            value: 'alice@foo.com',
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].name, 'alice');
    assertEq(resp.results.r.stats.index_used, 'index2');
  });

  test('functional: UPPER lookup', async () => {
    const db = await fixtures.setupDb(client, 'fn_upper', ['t']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'code_upper',
          table: 't',
          fields: [['code']],
          index_type: 'functional',
          functional_op: 'upper',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w: { insert_into: 't', values: [{ code: 'abc123', tag: 'first' }] },
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 't',
          where: {
            op: 'computed',
            expr_op: 'upper',
            field: ['code'],
            cmp: 'eq',
            value: 'ABC123',
          },
        },
      },
    });
    assertEq(resp.results.r.records.length, 1);
    assertEq(resp.results.r.records[0].tag, 'first');
  });

  // ─────────────────────────────────────────────────────────────────────
  // Vector (HNSW)
  // ─────────────────────────────────────────────────────────────────────

  test('vector: HNSW cosine similarity top-k', async () => {
    const db = await fixtures.setupDb(client, 'vec_cosine', ['docs']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'vec_idx',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: 3,
          vector_metric: 'cosine',
        },
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'docs', values: [{ embedding: [1.0, 0.0, 0.0], label: 'x' }] },
        w2: { insert_into: 'docs', values: [{ embedding: [0.0, 1.0, 0.0], label: 'y' }] },
        w3: { insert_into: 'docs', values: [{ embedding: [0.95, 0.1, 0.0], label: 'x_near' }] },
        w4: { insert_into: 'docs', values: [{ embedding: [0.0, 0.0, 1.0], label: 'z' }] },
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: [1.0, 0.0, 0.0],
            k: 2,
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 2);
    const labels = recs.map((r) => r.label);
    assert(labels.includes('x'), `expected 'x' in top-2: ${JSON.stringify(labels)}`);
    // The two closest should be 'x' and 'x_near'
    assert(labels.includes('x_near'), `expected 'x_near' in top-2: ${JSON.stringify(labels)}`);
    // HNSW vector index uses the ranked path.
    assertEq(resp.results.r.stats.index_used, 'index2_ranked');
  });

  test('vector: L2 metric', async () => {
    const db = await fixtures.setupDb(client, 'vec_l2', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['e']],
          index_type: 'vector',
          vector_dim: 2,
          vector_metric: 'l2',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: { insert_into: 'docs', values: [{ e: [0.0, 0.0], tag: 'origin' }] },
        w2: { insert_into: 'docs', values: [{ e: [3.0, 4.0], tag: 'far' }] },
        w3: { insert_into: 'docs', values: [{ e: [0.5, 0.5], tag: 'close' }] },
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['e'],
            query: [0.0, 0.0],
            k: 2,
          },
        },
      },
    });
    const labels = resp.results.r.records.map((r) => r.tag);
    assertEq(labels.length, 2);
    assert(labels.includes('origin'), `origin in top-2: ${JSON.stringify(labels)}`);
    assert(labels.includes('close'), `close in top-2: ${JSON.stringify(labels)}`);
  });
};
