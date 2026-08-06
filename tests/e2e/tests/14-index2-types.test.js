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

const { ddl, Query, write, filter } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {
  // ─────────────────────────────────────────────────────────────────────
  // FTS
  // ─────────────────────────────────────────────────────────────────────

  test('fts: create index, insert, AND query', async () => {
    const db = await fixtures.setupDb(client, 'fts_and', ['posts']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('body_fts', 'posts', [['body']], {
          index_type: 'fts',
          fts_tokenizer: 'whitespace',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('posts', { body: 'hello rust world' }),
        w2: write.insert('posts', { body: 'rust is great' }),
        w3: write.insert('posts', { body: 'hello python' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'hello world', 'and')).build(),
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
        i: ddl.createIndex('body_fts', 'posts', [['body']], { index_type: 'fts' }),
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('posts', { body: 'apple orange' }),
        w2: write.insert('posts', { body: 'banana pear' }),
        w3: write.insert('posts', { body: 'cherry grape' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'apple banana', 'or')).build(),
      },
    });
    assertEq(resp.results.r.records.length, 2);
  });

  test('fts: case-insensitive tokenization', async () => {
    const db = await fixtures.setupDb(client, 'fts_case', ['posts']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('b', 'posts', [['body']], { index_type: 'fts' }),
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: { w: write.insert('posts', { body: 'HELLO World' }) },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'hello WORLD', 'and')).build(),
      },
    });
    assertEq(resp.results.r.records.length, 1);
  });

  test('fts: brute-force fallback without index', async () => {
    const db = await fixtures.setupDb(client, 'fts_brute', ['posts']);
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('posts', { body: 'hello world' }),
        w2: write.insert('posts', { body: 'no match' }),
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'hello', 'and')).build(),
      },
    });
    assertEq(resp.results.r.records.length, 1);
  });

  test('fts: unicode tokenizer accepted and matches', async () => {
    const db = await fixtures.setupDb(client, 'fts_unicode', ['posts']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('body_fts_uni', 'posts', [['body']], {
          index_type: 'fts',
          fts_tokenizer: 'unicode',
        }),
      },
    });

    // "alpha,beta" has NO whitespace between the two words. The unicode
    // tokenizer splits on the comma (a non-alphanumeric boundary) into
    // ["alpha","beta"], whereas the whitespace tokenizer would keep it
    // as a single token "alpha,beta" — so a bare "beta" query only
    // matches because the unicode boundary-splitting actually happened.
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('posts', { body: 'alpha,beta gamma' }),
        w2: write.insert('posts', { body: 'delta epsilon' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'beta', 'and')).build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].body, 'alpha,beta gamma');
    // BM25-ranked FTS index path.
    assertEq(resp.results.r.stats.index_used, 'index2_ranked');
  });

  test('fts: language hint accepted (no-op today)', async () => {
    const db = await fixtures.setupDb(client, 'fts_lang', ['posts']);

    // fts_language is stored but currently NOT consumed for any
    // tokenization/stemming behaviour — the only honest assertion is
    // that create_index accepts it without error and FTS still matches.
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('body_fts_lang', 'posts', [['body']], {
          index_type: 'fts',
          fts_language: 'en',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w: write.insert('posts', { body: 'hello world' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('posts').where(filter.fts('body', 'hello', 'and')).build(),
      },
    });
    assertEq(resp.results.r.records.length, 1);
    assertEq(resp.results.r.stats.index_used, 'index2_ranked');
  });

  // ─────────────────────────────────────────────────────────────────────
  // Functional
  // ─────────────────────────────────────────────────────────────────────

  test('functional: LOWER(email) = lookup', async () => {
    const db = await fixtures.setupDb(client, 'fn_lower', ['users']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('email_lower', 'users', [['email']], {
          index_type: 'functional',
          functional_op: 'lower',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('users', { email: 'Alice@FOO.com', name: 'alice' }),
        w2: write.insert('users', { email: 'BOB@bar.org', name: 'bob' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('users')
          .where(filter.computed('lower', 'email', 'eq', 'alice@foo.com'))
          .build(),
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
        i: ddl.createIndex('code_upper', 't', [['code']], {
          index_type: 'functional',
          functional_op: 'upper',
        }),
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w: write.insert('t', { code: 'abc123', tag: 'first' }),
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('t')
          .where(filter.computed('upper', 'code', 'eq', 'ABC123'))
          .build(),
      },
    });
    assertEq(resp.results.r.records.length, 1);
    assertEq(resp.results.r.records[0].tag, 'first');
  });

  test('functional: TRIM(field) = lookup', async () => {
    const db = await fixtures.setupDb(client, 'fn_trim', ['users']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('email_trim', 'users', [['email']], {
          index_type: 'functional',
          functional_op: 'trim',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('users', { email: '  alice@foo.com  ', name: 'alice' }),
        w2: write.insert('users', { email: 'bob@bar.org', name: 'bob' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('users')
          .where(filter.computed('trim', 'email', 'eq', 'alice@foo.com'))
          .build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].name, 'alice');
    assertEq(resp.results.r.stats.index_used, 'index2');
  });

  test('functional: LENGTH(field) = lookup', async () => {
    const db = await fixtures.setupDb(client, 'fn_length', ['t']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('code_len', 't', [['code']], {
          index_type: 'functional',
          functional_op: 'length',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('t', { code: 'abc', tag: 'short' }), // length 3
        w2: write.insert('t', { code: 'abcdef', tag: 'long' }), // length 6
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('t')
          .where(filter.computed('length', 'code', 'eq', 6))
          .build(),
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].tag, 'long');
    assertEq(resp.results.r.stats.index_used, 'index2');
  });

  // ─────────────────────────────────────────────────────────────────────
  // Vector (HNSW)
  // ─────────────────────────────────────────────────────────────────────

  test('vector: HNSW cosine similarity top-k', async () => {
    const db = await fixtures.setupDb(client, 'vec_cosine', ['docs']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('vec_idx', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: 3,
          vector_metric: 'cosine',
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('docs', { embedding: [1.0, 0.0, 0.0], label: 'x' }),
        w2: write.insert('docs', { embedding: [0.0, 1.0, 0.0], label: 'y' }),
        w3: write.insert('docs', { embedding: [0.95, 0.1, 0.0], label: 'x_near' }),
        w4: write.insert('docs', { embedding: [0.0, 0.0, 1.0], label: 'z' }),
      },
    });

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('docs')
          .where(filter.vectorSimilarity('embedding', [1.0, 0.0, 0.0], 2))
          .build(),
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
        i: ddl.createIndex('v', 'docs', [['e']], {
          index_type: 'vector',
          vector_dim: 2,
          vector_metric: 'l2',
        }),
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('docs', { e: [0.0, 0.0], tag: 'origin' }),
        w2: write.insert('docs', { e: [3.0, 4.0], tag: 'far' }),
        w3: write.insert('docs', { e: [0.5, 0.5], tag: 'close' }),
      },
    });
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('docs').where(filter.vectorSimilarity('e', [0.0, 0.0], 2)).build(),
      },
    });
    const labels = resp.results.r.records.map((r) => r.tag);
    assertEq(labels.length, 2);
    assert(labels.includes('origin'), `origin in top-2: ${JSON.stringify(labels)}`);
    assert(labels.includes('close'), `close in top-2: ${JSON.stringify(labels)}`);
  });

  // ─────────────────────────────────────────────────────────────────────
  // Covering (include) sorted index — btree family, not index2.
  //
  // A sorted index with `include` stores the included field's value
  // directly in the index posting, so a range query that only projects
  // included fields is served entirely from the index (no data-store
  // fetch). The ONLY observable proof is `stats.index_used` ending in
  // `_covering` (read_index_scan.rs line 172/204).
  // ─────────────────────────────────────────────────────────────────────

  test('covering: sorted index with include serves range query from index only', async () => {
    const db = await fixtures.setupDb(client, 'cover_inc', ['scores']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('score_cover', 'scores', [['score']], {
          sorted: true,
          include: [['label']],
        }),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('scores', { score: 10, label: 'ten', extra: 'a' }),
        w2: write.insert('scores', { score: 20, label: 'twenty', extra: 'b' }),
        w3: write.insert('scores', { score: 30, label: 'thirty', extra: 'c' }),
        w4: write.insert('scores', { score: 40, label: 'forty', extra: 'd' }),
        w5: write.insert('scores', { score: 50, label: 'fifty', extra: 'e' }),
      },
    });

    // Range query projecting ONLY the included field — triggers the
    // covering index-only path (all SELECT items present in
    // included_fields, no residual, no order_by/group_by/agg).
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('scores')
          .select(['label'])
          .where(filter.between('score', 20, 40))
          .build(),
      },
    });

    const labels = resp.results.r.records.map((r) => r.label).sort();
    assertEq(labels.length, 3);
    assert(labels.includes('twenty'), `twenty missing: ${JSON.stringify(labels)}`);
    assert(labels.includes('thirty'), `thirty missing: ${JSON.stringify(labels)}`);
    assert(labels.includes('forty'), `forty missing: ${JSON.stringify(labels)}`);

    // Covering path proof: index_used must end with `_covering`.
    const idx = resp.results.r.stats.index_used;
    assert(
      typeof idx === 'string' && idx.endsWith('_covering'),
      `expected index_used ending in '_covering', got: ${JSON.stringify(idx)}`,
    );
  });

  test('covering: include on non-sorted index is rejected', async () => {
    const db = await fixtures.setupDb(client, 'cover_neg', ['t']);

    // #990 added a client-side `include`-without-`sorted` guard to the TS
    // builder's `createIndex()`, so `ddl.createIndex(...)` now throws
    // SYNCHRONOUSLY before any wire round-trip (assertThrows catches sync
    // throws too). The regex is loosened (`include.*only valid…`) to tolerate
    // the backtick in the client-side message ("`include` is only valid for
    // sorted indexes"). The server's own identical check
    // (admin_table_index.rs ~line 472: "include is only valid for sorted
    // indexes") is now covered separately by the Rust e2e test
    // `server_rejects_include_without_sorted` in
    // create_index_validation_e2e.rs.
    const err = await assertThrows(() =>
      client.execute(db, {
        id: 'mk-bad',
        queries: {
          i: ddl.createIndex('bad_include', 't', [['score']], {
            include: [['label']],
          }),
        },
      }),
    );
    assert(
      /include.*only valid for sorted indexes/i.test(err.message),
      `expected include-rejection error, got: ${err.message}`,
    );
  });

  // ─────────────────────────────────────────────────────────────────────
  // Composite (multi-field) regular index — btree family, not index2.
  //
  // A REGULAR (non-sorted, non-unique) index accepts multiple columns as
  // one composite key. The planner matches an And(eq, eq) filter against
  // the composite index when every path is covered by exactly one Eq
  // (read_planner.rs ~line 232). Sorted indexes explicitly REJECT
  // multi-field today — "composite TBD" (admin_table_index.rs ~line 392).
  // ─────────────────────────────────────────────────────────────────────

  test('composite: regular multi-field index serves AND equality query', async () => {
    const db = await fixtures.setupDb(client, 'comp_reg', ['t']);

    await client.execute(db, {
      id: 'mk',
      queries: {
        i: ddl.createIndex('ab_comp', 't', [['a'], ['b']]),
      },
    });

    await client.execute(db, {
      id: 'ins',
      queries: {
        w1: write.insert('t', { a: 1, b: 'x', name: 'r1' }),
        w2: write.insert('t', { a: 1, b: 'y', name: 'r2' }),
        w3: write.insert('t', { a: 2, b: 'x', name: 'r3' }),
        w4: write.insert('t', { a: 2, b: 'y', name: 'r4' }),
      },
    });

    // And(eq(a,2), eq(b,'x')) — only r3 matches both.
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: Query.from('t')
          .where(filter.and(filter.eq('a', 2), filter.eq('b', 'x')))
          .build(),
      },
    });

    const recs = resp.results.r.records;
    assertEq(recs.length, 1);
    assertEq(recs[0].name, 'r3');

    // The composite index was used (index_used = index name string).
    const idx = resp.results.r.stats.index_used;
    assert(
      idx === 'ab_comp',
      `expected index_used 'ab_comp', got: ${JSON.stringify(idx)}`,
    );
  });

  test('composite: sorted multi-field index is rejected (single-field scalar only)', async () => {
    const db = await fixtures.setupDb(client, 'comp_sorted_neg', ['t']);

    // Sorted + multi-field must be rejected — the current limitation is
    // explicitly enforced (admin_table_index.rs ~line 392-397).
    const err = await assertThrows(() =>
      client.execute(db, {
        id: 'mk-bad',
        queries: {
          i: ddl.createIndex('bad_comp_sorted', 't', [['a'], ['b']], {
            sorted: true,
          }),
        },
      }),
    );
    // #1036: the client-side query builder now validates field count
    // BEFORE the request ever reaches the server (mirrors the server's own
    // "Sorted index requires exactly one field (composite TBD)" check —
    // see crates/shamir-client-ts/src/core/builders/ddl.ts and
    // crates/shamir-query-builder/src/ddl/create_index_build_error.rs), so
    // this test now observes the CLIENT's message, not the server's. Match
    // the substring stable across both variants instead of the
    // server-only "composite TBD" phrase.
    assert(
      /requires exactly one field/i.test(err.message),
      `expected a "requires exactly one field" rejection, got: ${err.message}`,
    );
  });
};
