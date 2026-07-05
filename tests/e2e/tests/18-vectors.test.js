/**
 * E2E tests for the full vector stack (V5–V6.1 campaign):
 *
 *   Node client → WS/TCP → shamir-server → engine → HNSW
 *
 * Covers the new vector capabilities introduced across the vector campaign:
 *   1. DDL with all new options: index_type "vector", vector_dim,
 *      vector_metric (cosine / l2 / dot), vector_quantization "sq8" (#411).
 *   2. Insert + ANN top-k: clustered vectors, vector_similarity returns the
 *      nearest neighbours ordered by distance.
 *   3. Per-query ef_search + oversample (#399): larger ef_search does not
 *      degrade recall; invalid values surface a clear error.
 *   4. Filtered ANN (#404–405): And([VectorSimilarity, residual-predicate])
 *      — only filtered records are returned, top-k is correct.
 *   5. Sequential tx invariants on the full stack (#416/#420 lite): two
 *      inserts → each finds itself as its own top-1; delete a row → its rid
 *      disappears from ANN output.
 *   6. Quantised table e2e (#411/#412): index with quantization "sq8", insert
 *      > fit threshold (256+) vectors in batches, ANN still returns correct
 *      top-k (soft recall: own vector in top-3).
 *
 * Wire field reference (confirmed by reading the Rust source):
 *   CreateIndexOp (crates/shamir-query-types/src/admin/types/index_ops.rs):
 *     index_type: "vector"
 *     vector_dim: u32
 *     vector_metric: "cosine" | "l2" | "dot"
 *     vector_quantization: "sq8"   (V5.2 #411, Option<String>, omitted when None)
 *   Filter::VectorSimilarity (crates/shamir-query-types/src/filter/filter_enum.rs):
 *     { op: "vector_similarity", field, query: [f32...], k: u32,
 *       ef_search?: u32, oversample?: f32 }   (ef_search/oversample omitted when None)
 *   Filtered ANN shape (crates/shamir-engine/src/table/filtered_vector.rs):
 *     { op: "and", filters: [ {vector_similarity op}, {residual predicate} ] }
 *   index_used labels:
 *     bare VectorSimilarity → "index2_ranked"
 *     And([VectorSimilarity, residual]) → "filtered_vector_scan"
 *
 * NOTE on the dot metric: the server treats `dot` as inner-product similarity
 * (higher = closer). The HNSW adapter maps all three metrics internally.
 */

'use strict';

// ─────────────────────────────────────────────────────────────────────────────
// Vector data helpers
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Build a unit vector along a single axis: a 1.0 in position `axis` and 0
 * elsewhere. Two such vectors on different axes are orthogonal (cosine = 0).
 */
function axisVector(dim, axis) {
  const v = new Array(dim).fill(0.0);
  v[axis] = 1.0;
  return v;
}

/**
 * Build a vector near the given axis: mostly the axis component with a small
 * jitter on the next component. Cosine to the pure axis vector is close to 1.
 */
function nearAxisVector(dim, axis, jitter = 0.1) {
  const v = new Array(dim).fill(0.0);
  v[axis] = 1.0;
  const next = (axis + 1) % dim;
  v[next] = jitter;
  return v;
}

/**
 * Insert `count` vectors into `table` in batches of `batchSize`, each batch
 * being a single insert_into with multiple values. Returns when all batches
 * are committed. Uses axis clusters: cluster i has vectors near axisVector(dim, i).
 *
 * Each record carries: { id, embedding, cluster } so we can delete / filter on
 * them later.
 */
async function insertClusteredBatched(client, db, table, count, dim, batchSize) {
  let inserted = 0;
  let clusterIdx = 0;
  while (inserted < count) {
    const take = Math.min(batchSize, count - inserted);
    const values = [];
    for (let i = 0; i < take; i += 1) {
      // Alternate: pure axis vector vs near-axis vector, cycling clusters.
      const vec =
        inserted % 2 === 0
          ? axisVector(dim, clusterIdx % dim)
          : nearAxisVector(dim, clusterIdx % dim, 0.05 + ((inserted % 7) * 0.01));
      values.push({
        id: `v-${inserted}`,
        embedding: vec,
        cluster: clusterIdx % dim,
      });
      inserted += 1;
      if (inserted % 10 === 0) clusterIdx += 1;
    }
    // eslint-disable-next-line no-await-in-loop
    await client.execute(db, {
      id: `ins-batch-${inserted}`,
      queries: {
        ins: { insert_into: table, values },
      },
    });
  }
  return inserted;
}

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {
  // ─────────────────────────────────────────────────────────────────────
  // (1) DDL with all new options — vector_quantization "sq8" (#411)
  // ─────────────────────────────────────────────────────────────────────

  test('ddl: vector index with sq8 quantization is accepted', async () => {
    const db = await fixtures.setupDb(client, 'vec_ddl_sq8', ['docs']);

    const resp = await client.execute(db, {
      id: 'mk-sq8',
      queries: {
        i: {
          create_index: 'vec_sq8',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: 8,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        },
      },
    });
    // A successful create_index returns a record; no error thrown.
    assert(resp && resp.results, 'create_index with sq8 returned a result');
    assert(resp.results.i, 'create_index result for the sq8 index is present');
  });

  test('ddl: vector index without quantization still accepted (back-compat)', async () => {
    const db = await fixtures.setupDb(client, 'vec_ddl_plain', ['docs']);
    const resp = await client.execute(db, {
      id: 'mk-plain',
      queries: {
        i: {
          create_index: 'vec_plain',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: 4,
          vector_metric: 'cosine',
        },
      },
    });
    assert(resp.results.i, 'plain vector index created');
  });

  test('ddl: dot metric accepted', async () => {
    const db = await fixtures.setupDb(client, 'vec_ddl_dot', ['docs']);
    const resp = await client.execute(db, {
      id: 'mk-dot',
      queries: {
        i: {
          create_index: 'vec_dot',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: 4,
          vector_metric: 'dot',
        },
      },
    });
    assert(resp.results.i, 'dot-metric vector index created');
  });

  // ─────────────────────────────────────────────────────────────────────
  // (2) Insert + ANN top-k (cosine) — clustered vectors
  // ─────────────────────────────────────────────────────────────────────

  test('ann: cosine top-k returns nearest in distance order', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_ann_cos', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });

    // 30 vectors across 8 clusters. Each cluster has a "pure" axis vector and
    // a few near-axis vectors. Querying axisVector(dim, 0) should rank cluster-0
    // vectors highest.
    await insertClusteredBatched(client, db, 'docs', 30, dim, 10);

    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: axisVector(dim, 0),
            k: 5,
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 5);
    // Every returned record must be from cluster 0 (nearest to axis 0).
    for (const r of recs) {
      assertEq(r.cluster, 0, `expected cluster 0, got ${r.cluster} (id=${r.id})`);
    }
    // Ranked path.
    assertEq(resp.results.r.stats.index_used, 'index2_ranked');
  });

  test('ann: l2 metric ranks by Euclidean distance', async () => {
    const dim = 4;
    const db = await fixtures.setupDb(client, 'vec_ann_l2', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'l2',
        },
      },
    });

    // Under L2, [0,0,0,0] is closest to small-magnitude vectors.
    await client.execute(db, {
      id: 'ins',
      queries: {
        a: { insert_into: 'docs', values: [{ id: 'origin', embedding: [0, 0, 0, 0], cluster: 0 }] },
        b: { insert_into: 'docs', values: [{ id: 'near', embedding: [0.1, 0.1, 0.1, 0.1], cluster: 0 }] },
        c: { insert_into: 'docs', values: [{ id: 'mid', embedding: [1, 1, 1, 1], cluster: 1 }] },
        d: { insert_into: 'docs', values: [{ id: 'far', embedding: [5, 5, 5, 5], cluster: 2 }] },
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
            query: [0, 0, 0, 0],
            k: 2,
          },
        },
      },
    });
    const ids = resp.results.r.records.map((r) => r.id);
    assertEq(ids.length, 2);
    assert(ids.includes('origin'), `origin in top-2: ${JSON.stringify(ids)}`);
    assert(ids.includes('near'), `near in top-2: ${JSON.stringify(ids)}`);
  });

  // ─────────────────────────────────────────────────────────────────────
  // (3) Per-query ef_search + oversample (#399)
  // ─────────────────────────────────────────────────────────────────────

  test('ef_search: larger ef does not degrade recall (top-1 stable)', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_ef', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });
    await insertClusteredBatched(client, db, 'docs', 40, dim, 10);

    // The exact target vector exists in the set: cluster-0 axis vector.
    const target = axisVector(dim, 0);

    // Small ef (still valid): should still find cluster-0 vectors.
    const small = await client.execute(db, {
      id: 'q-small',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: target,
            k: 3,
            ef_search: 16,
          },
        },
      },
    });
    const smallIds = small.results.r.records.map((r) => r.id);
    assertEq(small.results.r.records.length, 3);

    // Large ef: recall should not be worse (all cluster-0 still found).
    const large = await client.execute(db, {
      id: 'q-large',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: target,
            k: 3,
            ef_search: 256,
          },
        },
      },
    });
    const largeIds = large.results.r.records.map((r) => r.id);
    assertEq(large.results.r.records.length, 3);

    // The set of top-3 ids with large ef must be a SUPERSET (≥ same recall) of
    // the small-ef set. We check that every id found by small-ef is also found
    // by large-ef — larger exploration width cannot drop a true neighbour.
    for (const id of smallIds) {
      assert(
        largeIds.includes(id),
        `large ef_search lost id ${id} present with small ef (recall regression)`
      );
    }
    // All returned records are cluster 0.
    for (const r of large.results.r.records) {
      assertEq(r.cluster, 0);
    }
  });

  test('ef_search: very large value is clamped, not rejected', async () => {
    const dim = 4;
    const db = await fixtures.setupDb(client, 'vec_ef_clamp', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        a: { insert_into: 'docs', values: [{ id: 'a', embedding: [1, 0, 0, 0], cluster: 0 }] },
        b: { insert_into: 'docs', values: [{ id: 'b', embedding: [0, 1, 0, 0], cluster: 1 }] },
      },
    });

    // ef_search far above MAX_EF_SEARCH (10_000) must clamp, not error.
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: [1, 0, 0, 0],
            k: 1,
            ef_search: 999_999_999,
          },
        },
      },
    });
    assertEq(resp.results.r.records.length, 1);
    assertEq(resp.results.r.records[0].id, 'a');
  });

  test('oversample: explicit oversample on bare vector_similarity is accepted', async () => {
    const dim = 4;
    const db = await fixtures.setupDb(client, 'vec_oversample', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        a: { insert_into: 'docs', values: [{ id: 'a', embedding: [1, 0, 0, 0], cluster: 0 }] },
        b: { insert_into: 'docs', values: [{ id: 'b', embedding: [0.9, 0.1, 0, 0], cluster: 0 }] },
      },
    });

    // oversample on a BARE vector_similarity is accepted on the wire; the
    // engine consumes it only for the filtered path, but it must not error.
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: [1, 0, 0, 0],
            k: 2,
            oversample: 3.0,
          },
        },
      },
    });
    assertEq(resp.results.r.records.length, 2);
  });

  // ─────────────────────────────────────────────────────────────────────
  // (4) Filtered ANN (#404–405): And([VectorSimilarity, residual])
  // ─────────────────────────────────────────────────────────────────────

  test('filtered ann: And(vector_similarity, eq) returns only matching rows', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_filtered', ['docs']);
    await client.execute(db, {
      id: 'mk-vec',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });

    // Insert vectors in 3 clusters, each with a `group` tag.
    const values = [];
    for (let c = 0; c < 3; c += 1) {
      // 4 vectors per cluster, alternating pure + near.
      for (let j = 0; j < 4; j += 1) {
        const vec = j % 2 === 0 ? axisVector(dim, c) : nearAxisVector(dim, c, 0.05 + j * 0.01);
        values.push({ id: `c${c}-${j}`, embedding: vec, group: `g${c}`, cluster: c });
      }
    }
    await client.execute(db, {
      id: 'ins',
      queries: { ins: { insert_into: 'docs', values } },
    });

    // Query axis 0 BUT restrict to group "g1" — the nearest in g1 must be
    // cluster-1 vectors (which are near axis 1, not axis 0), but the FILTER
    // guarantees only g1 rows are returned.
    const resp = await client.execute(db, {
      id: 'q-filtered',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'and',
            filters: [
              {
                op: 'vector_similarity',
                field: ['embedding'],
                query: axisVector(dim, 1),
                k: 3,
              },
              { op: 'eq', field: ['group'], value: 'g1' },
            ],
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 3);
    // Every returned record passed the residual filter.
    for (const r of recs) {
      assertEq(r.group, 'g1', `residual filter violated: group=${r.group}`);
    }
    // The filtered-vector scan path was used.
    assertEq(
      resp.results.r.stats.index_used,
      'filtered_vector_scan',
      `expected filtered_vector_scan, got ${resp.results.r.stats.index_used}`
    );
  });

  test('filtered ann: predicate matching nothing terminates with ≤ k', async () => {
    const dim = 4;
    const db = await fixtures.setupDb(client, 'vec_filtered_empty', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });
    await client.execute(db, {
      id: 'ins',
      queries: {
        a: { insert_into: 'docs', values: [{ id: 'a', embedding: [1, 0, 0, 0], group: 'x' }] },
        b: { insert_into: 'docs', values: [{ id: 'b', embedding: [0, 1, 0, 0], group: 'x' }] },
      },
    });

    // Predicate group = "nonexistent" matches nothing; must return empty
    // (NOT infinite-retry, NOT hang).
    const resp = await client.execute(db, {
      id: 'q-empty',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'and',
            filters: [
              { op: 'vector_similarity', field: ['embedding'], query: [1, 0, 0, 0], k: 5 },
              { op: 'eq', field: ['group'], value: 'nonexistent' },
            ],
          },
        },
      },
    });
    assertEq(resp.results.r.records.length, 0);
  });

  // ─────────────────────────────────────────────────────────────────────
  // (5) Sequential tx invariants on the full stack (#416/#420 lite)
  // ─────────────────────────────────────────────────────────────────────

  test('seq: each inserted vector is its own top-1', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_seq', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });

    // Insert two distinct vectors in separate batches.
    const vecA = axisVector(dim, 0);
    const vecB = axisVector(dim, 1);
    await client.execute(db, {
      id: 'ins-a',
      queries: { ins: { insert_into: 'docs', values: [{ id: 'a', embedding: vecA }] } },
    });
    await client.execute(db, {
      id: 'ins-b',
      queries: { ins: { insert_into: 'docs', values: [{ id: 'b', embedding: vecB }] } },
    });

    // Querying vecA must rank 'a' as top-1.
    const qA = await client.execute(db, {
      id: 'q-a',
      queries: {
        r: {
          from: 'docs',
          where: { op: 'vector_similarity', field: ['embedding'], query: vecA, k: 1 },
        },
      },
    });
    assertEq(qA.results.r.records.length, 1);
    assertEq(qA.results.r.records[0].id, 'a');

    // Querying vecB must rank 'b' as top-1.
    const qB = await client.execute(db, {
      id: 'q-b',
      queries: {
        r: {
          from: 'docs',
          where: { op: 'vector_similarity', field: ['embedding'], query: vecB, k: 1 },
        },
      },
    });
    assertEq(qB.results.r.records.length, 1);
    assertEq(qB.results.r.records[0].id, 'b');
  });

  test('seq: delete removes rid from ANN output', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_seq_del', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        },
      },
    });

    // Insert three vectors on distinct axes.
    const vecA = axisVector(dim, 0);
    const vecB = axisVector(dim, 1);
    const vecC = axisVector(dim, 2);
    await client.execute(db, {
      id: 'ins',
      queries: {
        a: { insert_into: 'docs', values: [{ id: 'a', embedding: vecA }] },
        b: { insert_into: 'docs', values: [{ id: 'b', embedding: vecB }] },
        c: { insert_into: 'docs', values: [{ id: 'c', embedding: vecC }] },
      },
    });

    // Pre-delete: top-1 for vecA is 'a'.
    const before = await client.execute(db, {
      id: 'q-before',
      queries: {
        r: {
          from: 'docs',
          where: { op: 'vector_similarity', field: ['embedding'], query: vecA, k: 1 },
        },
      },
    });
    assertEq(before.results.r.records[0].id, 'a');

    // Delete 'a'.
    await client.execute(db, {
      id: 'del-a',
      queries: { d: { delete_from: 'docs', where: { op: 'eq', field: ['id'], value: 'a' } } },
    });

    // Post-delete: top-1 for vecA must NOT be 'a'.
    const after = await client.execute(db, {
      id: 'q-after',
      queries: {
        r: {
          from: 'docs',
          where: { op: 'vector_similarity', field: ['embedding'], query: vecA, k: 1 },
        },
      },
    });
    assertEq(after.results.r.records.length, 1);
    assert(
      after.results.r.records[0].id !== 'a',
      `deleted rid 'a' still present in ANN: ${after.results.r.records[0].id}`
    );
  });

  // ─────────────────────────────────────────────────────────────────────
  // (6) Quantised table e2e (#411/#412): sq8 index, > fit threshold (256)
  // ─────────────────────────────────────────────────────────────────────

  test('sq8: insert >256 vectors, ANN returns correct top-k (soft recall)', async () => {
    const dim = 16;
    const db = await fixtures.setupDb(client, 'vec_sq8_fit', ['docs']);
    await client.execute(db, {
      id: 'mk-sq8',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        },
      },
    });

    // The fit threshold is 256 (crates/shamir-index/src/vector/hnsw_adapter.rs,
    // FIT_THRESHOLD == BRUTE_FORCE_MAX == 256). Insert 280 to cross it and
    // trigger the u8-graph fit.
    const total = 280;
    await insertClusteredBatched(client, db, 'docs', total, dim, 32);

    // Pick a known vector — the first one we inserted — and verify it appears
    // in its own top-3 (soft recall: SQ8 dequant-rescore can reorder near-ties
    // but the exact-match vector must survive).
    const probeId = 'v-0';
    // Reconstruct the exact vector for v-0: it is an axis vector on axis 0.
    const probeVec = axisVector(dim, 0);

    const resp = await client.execute(db, {
      id: 'q-probe',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'vector_similarity',
            field: ['embedding'],
            query: probeVec,
            k: 3,
            ef_search: 128,
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 3);
    const ids = recs.map((r) => r.id);
    assert(
      ids.includes(probeId),
      `own vector ${probeId} not in top-3 after sq8 fit: ${JSON.stringify(ids)}`
    );
  });

  test('sq8: filtered ANN works after fit', async () => {
    const dim = 8;
    const db = await fixtures.setupDb(client, 'vec_sq8_filtered', ['docs']);
    await client.execute(db, {
      id: 'mk',
      queries: {
        i: {
          create_index: 'v',
          table: 'docs',
          fields: [['embedding']],
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        },
      },
    });

    // 270 vectors, each tagged with cluster = its axis.
    await insertClusteredBatched(client, db, 'docs', 270, dim, 32);

    // Filtered ANN: nearest to axis 0 within cluster 0.
    const resp = await client.execute(db, {
      id: 'q',
      queries: {
        r: {
          from: 'docs',
          where: {
            op: 'and',
            filters: [
              {
                op: 'vector_similarity',
                field: ['embedding'],
                query: axisVector(dim, 0),
                k: 3,
                ef_search: 128,
              },
              { op: 'eq', field: ['cluster'], value: 0 },
            ],
          },
        },
      },
    });
    const recs = resp.results.r.records;
    assertEq(recs.length, 3);
    for (const r of recs) {
      assertEq(r.cluster, 0, `filtered ann returned wrong cluster: ${r.cluster}`);
    }
    assertEq(
      resp.results.r.stats.index_used,
      'filtered_vector_scan',
      `expected filtered_vector_scan after sq8 fit, got ${resp.results.r.stats.index_used}`
    );
  });
};
