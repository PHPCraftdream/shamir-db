/**
 * End-to-end vector similarity search test (V6.2 campaign — task #414).
 *
 * Exercises the full vector stack through the TYPED TS builder API
 * (ddl.createIndex / filter.vectorSimilarity / filter.and / filter.eq /
 * write.insert / write.del / Query.from().where().build()), against a
 * real release-binary shamir-server brought up by `e2e-harness.ts`.
 *
 * Coverage (mirrors the Node-side `tests/e2e/tests/18-vectors.test.js`):
 *   1. Realistic dims (64/128) + all 3 metrics (cosine / l2 / dot) — index
 *      create + ANN top-k with order / cluster assertions.
 *   2. DDL with `vector_quantization: "sq8"` via builder + back-compat
 *      index without quantization.
 *   3. Per-query `efSearch` / `oversample` via builder — recall-superset
 *      assertion (larger ef does not drop ids from a smaller-ef result),
 *      clamp of an enormous ef value.
 *   4. Filtered ANN via builder (`and(vectorSimilarity, eq)`): only
 *      filter-passing rows returned, `stats.index_used ==
 *      'filtered_vector_scan'`; an empty residual predicate terminates
 *      with 0 records.
 *   5. Insert → delete → ANN: a deleted id disappears from the result set.
 *   6. sq8 across the fit threshold (280+ vectors, dim 16): own vector in
 *      top-3 (soft recall), filtered ANN after fit works.
 *
 * The original 4-dim cosine smoke tests are preserved at the top.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, WireValue } from '../index.js';
import { Batch, Query, filter, ddl, write } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  setupDb,
  seed,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// ─── vector data helpers ─────────────────────────────────────────────────────

/**
 * Build a unit vector along a single axis: a 1.0 in position `axis` and 0
 * elsewhere. Two such vectors on different axes are orthogonal (cosine = 0).
 */
function axisVector(dim: number, axis: number): number[] {
  const v = new Array(dim).fill(0.0);
  v[axis] = 1.0;
  return v;
}

/**
 * Build a vector near the given axis: mostly the axis component with a small
 * jitter on the next component. Cosine to the pure axis vector is close to 1.
 */
function nearAxisVector(dim: number, axis: number, jitter = 0.1): number[] {
  const v = new Array(dim).fill(0.0);
  v[axis] = 1.0;
  const next = (axis + 1) % dim;
  v[next] = jitter;
  return v;
}

/**
 * Insert `count` vectors into `table` in batches of `batchSize`. Uses axis
 * clusters: cluster i has vectors near axisVector(dim, i).
 *
 * Each record carries: { id, embedding, cluster } so we can delete / filter
 * on them later. Uses the typed `write.insert` builder + `Batch`.
 */
async function insertClusteredBatched(
  client: ShamirClient,
  db: string,
  table: string,
  count: number,
  dim: number,
  batchSize: number,
): Promise<number> {
  let inserted = 0;
  let clusterIdx = 0;
  while (inserted < count) {
    const take = Math.min(batchSize, count - inserted);
    const values: Array<Record<string, WireValue>> = [];
    for (let i = 0; i < take; i += 1) {
      // Alternate: pure axis vector vs near-axis vector, cycling clusters.
      const vec =
        inserted % 2 === 0
          ? axisVector(dim, clusterIdx % dim)
          : nearAxisVector(dim, clusterIdx % dim, 0.05 + (inserted % 7) * 0.01);
      values.push({
        id: `v-${inserted}`,
        embedding: vec,
        cluster: clusterIdx % dim,
      });
      inserted += 1;
      if (inserted % 10 === 0) clusterIdx += 1;
    }
    // eslint-disable-next-line no-await-in-loop
    br(
      await Batch.create(`ins-batch-${inserted}`)
        .add('ins', write.insert(table, values))
        .execute(client, db),
    );
  }
  return inserted;
}

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e Vector — createIndex(vector) + insert + top-k similarity (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-vector] connection failed. Server logs:\n' + server!.logs());
        throw e;
      }
      db = await setupDb(client, 'vec', ['embeddings']);
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    it('vector: create index + insert + top-k similarity returns ordered results', async () => {
      // 1. Create a vector index on the "vec" field (4-dimensional, cosine metric).
      br(await Batch.create('mk-vec-idx')
        .add('i', ddl.createIndex('vec_emb', 'embeddings', [['vec']], {
          index_type: 'vector',
          vector_dim: 4,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      // 2. Insert records with 4D vectors.
      // v1 is very close to query, v2 is somewhat close, v3 is far.
      await seed(client!, db, 'embeddings', [
        { id: 'v1', vec: [1.0, 0.0, 0.0, 0.0], label: 'closest' },
        { id: 'v2', vec: [0.7, 0.7, 0.0, 0.0], label: 'middle' },
        { id: 'v3', vec: [0.0, 0.0, 0.0, 1.0], label: 'farthest' },
      ]);

      // 3. Query for top-2 nearest to [1, 0, 0, 0].
      const resp = br(await Batch.create('vec-sim')
        .add('q', Query.from('embeddings')
          .where(filter.vectorSimilarity('vec', [1.0, 0.0, 0.0, 0.0], 2))
          .build())
        .execute(client!, db));

      const records = resp.results.q.records;
      // Should return top-2 (closest first)
      expect(records.length).toBe(2);
      expect(records[0].id).toBe('v1');
      expect(records[1].id).toBe('v2');
    });

    it('vector: top-k=3 returns all records', async () => {
      const resp = br(await Batch.create('vec-all')
        .add('q', Query.from('embeddings')
          .where(filter.vectorSimilarity('vec', [1.0, 0.0, 0.0, 0.0], 3))
          .build())
        .execute(client!, db));

      const records = resp.results.q.records;
      expect(records.length).toBe(3);
      // v1 (closest), v2 (middle), v3 (farthest) by cosine to [1,0,0,0]
      expect(records[0].id).toBe('v1');
      expect(records[2].id).toBe('v3');
    });
  },
);

// ─── V6.2 extended scenarios (typed builder) ────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e Vector V6.2 — DDL + ANN + efSearch + filtered ANN + delete + sq8 (typed builder)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-vector-v62] connection failed. Server logs:\n' + server!.logs());
        throw e;
      }
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try { await client.close(); } catch { /* ok */ }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    // ─── (1) Realistic dims + all 3 metrics ──────────────────────────────────

    it('ann: cosine 64-dim top-k returns cluster-0 in distance order (ranked path)', async () => {
      const dim = 64;
      const db = await setupDb(client!, 'vec_cos64', ['docs']);

      br(await Batch.create('mk-cos64')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      await insertClusteredBatched(client!, db, 'docs', 30, dim, 10);

      const resp = br(await Batch.create('q-cos64')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', axisVector(dim, 0), 5))
          .build())
        .execute(client!, db));

      const recs = resp.results.r.records;
      expect(recs.length).toBe(5);
      // Every returned record must be from cluster 0 (nearest to axis 0).
      for (const r of recs) {
        expect(r.cluster).toBe(0);
      }
      // Bare vector_similarity uses the ranked index path.
      expect(resp.results.r.stats?.index_used).toBe('index2_ranked');
    });

    it('ann: l2 128-dim ranks by Euclidean distance (origin beats far points)', async () => {
      const dim = 128;
      const db = await setupDb(client!, 'vec_l2_128', ['docs']);

      br(await Batch.create('mk-l2')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'l2',
        }))
        .execute(client!, db));

      const zeros = new Array(dim).fill(0.0);
      const smalls = zeros.map((_, i) => (i === 0 ? 0.1 : 0.0));
      const mids = zeros.map((_, i) => (i < 4 ? 1.0 : 0.0));
      const fars = zeros.map(() => 5.0);

      br(await Batch.create('ins-l2')
        .add('a', write.insert('docs', [{ id: 'origin', embedding: zeros, cluster: 0 }]))
        .add('b', write.insert('docs', [{ id: 'near', embedding: smalls, cluster: 0 }]))
        .add('c', write.insert('docs', [{ id: 'mid', embedding: mids, cluster: 1 }]))
        .add('d', write.insert('docs', [{ id: 'far', embedding: fars, cluster: 2 }]))
        .execute(client!, db));

      const resp = br(await Batch.create('q-l2')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', zeros, 2))
          .build())
        .execute(client!, db));

      const ids = resp.results.r.records.map((r) => r.id);
      expect(ids.length).toBe(2);
      expect(ids).toContain('origin');
      expect(ids).toContain('near');
    });

    it('ann: dot metric on 8-dim ranks inner-product neighbours', async () => {
      const dim = 8;
      const db = await setupDb(client!, 'vec_dot', ['docs']);

      br(await Batch.create('mk-dot')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'dot',
        }))
        .execute(client!, db));

      // Under dot (inner-product, higher = closer), a vector parallel to the
      // query with large magnitude is closest. The query is [2,0,0,0,0,0,0,0].
      // [3,0,...] -> 6 ; [1,0,...] -> 2 ; orthogonal -> 0.
      const q = [2, 0, 0, 0, 0, 0, 0, 0];
      br(await Batch.create('ins-dot')
        .add('a', write.insert('docs', [{ id: 'big', embedding: [3, 0, 0, 0, 0, 0, 0, 0], cluster: 0 }]))
        .add('b', write.insert('docs', [{ id: 'med', embedding: [1, 0, 0, 0, 0, 0, 0, 0], cluster: 0 }]))
        .add('c', write.insert('docs', [{ id: 'zero', embedding: [0, 1, 0, 0, 0, 0, 0, 0], cluster: 1 }]))
        .execute(client!, db));

      const resp = br(await Batch.create('q-dot')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', q, 2))
          .build())
        .execute(client!, db));

      const recs = resp.results.r.records;
      expect(recs.length).toBe(2);
      // Higher inner-product first: 'big' (6) before 'med' (2).
      expect(recs[0].id).toBe('big');
      expect(recs[1].id).toBe('med');
    });

    // ─── (2) DDL: sq8 quantization via builder + back-compat ────────────────

    it('ddl: vector index with sq8 quantization via builder is accepted', async () => {
      const db = await setupDb(client!, 'vec_ddl_sq8', ['docs']);

      const resp = br(await Batch.create('mk-sq8')
        .add('i', ddl.createIndex('vec_sq8', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: 8,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        }))
        .execute(client!, db));

      // A successful create_index returns a result entry for the alias.
      expect(resp.results.i).toBeDefined();
    });

    it('ddl: vector index without quantization (back-compat) via builder', async () => {
      const db = await setupDb(client!, 'vec_ddl_plain', ['docs']);

      const resp = br(await Batch.create('mk-plain')
        .add('i', ddl.createIndex('vec_plain', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: 4,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      expect(resp.results.i).toBeDefined();
    });

    // ─── (3) Per-query efSearch / oversample via builder ────────────────────

    it('efSearch: larger ef does not drop ids from smaller-ef result (recall-superset)', async () => {
      const dim = 8;
      const db = await setupDb(client!, 'vec_ef', ['docs']);

      br(await Batch.create('mk-ef')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      await insertClusteredBatched(client!, db, 'docs', 40, dim, 10);

      const target = axisVector(dim, 0);

      const small = br(await Batch.create('q-ef-small')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', target, 3, { efSearch: 16 }))
          .build())
        .execute(client!, db));
      const smallIds = small.results.r.records.map((r) => r.id);
      expect(small.results.r.records.length).toBe(3);

      const large = br(await Batch.create('q-ef-large')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', target, 3, { efSearch: 256 }))
          .build())
        .execute(client!, db));
      const largeIds = large.results.r.records.map((r) => r.id);
      expect(large.results.r.records.length).toBe(3);

      // Larger exploration width cannot drop a true neighbour — every id
      // found by the small-ef query must still be present at large ef.
      for (const id of smallIds) {
        expect(largeIds).toContain(id);
      }
      // All returned records are cluster 0.
      for (const r of large.results.r.records) {
        expect(r.cluster).toBe(0);
      }
    });

    it('efSearch: huge value is clamped, not rejected', async () => {
      const dim = 4;
      const db = await setupDb(client!, 'vec_ef_clamp', ['docs']);

      br(await Batch.create('mk-clamp')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      br(await Batch.create('ins-clamp')
        .add('a', write.insert('docs', [{ id: 'a', embedding: [1, 0, 0, 0], cluster: 0 }]))
        .add('b', write.insert('docs', [{ id: 'b', embedding: [0, 1, 0, 0], cluster: 1 }]))
        .execute(client!, db));

      // ef_search far above MAX_EF_SEARCH (10_000) must clamp, not error.
      const resp = br(await Batch.create('q-clamp')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', [1, 0, 0, 0], 1, { efSearch: 999_999_999 }))
          .build())
        .execute(client!, db));

      expect(resp.results.r.records.length).toBe(1);
      expect(resp.results.r.records[0].id).toBe('a');
    });

    it('oversample: explicit oversample on bare vector_similarity is accepted', async () => {
      const dim = 4;
      const db = await setupDb(client!, 'vec_oversample', ['docs']);

      br(await Batch.create('mk-os')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      br(await Batch.create('ins-os')
        .add('a', write.insert('docs', [{ id: 'a', embedding: [1, 0, 0, 0], cluster: 0 }]))
        .add('b', write.insert('docs', [{ id: 'b', embedding: [0.9, 0.1, 0, 0], cluster: 0 }]))
        .execute(client!, db));

      // oversample on a BARE vector_similarity is accepted on the wire; the
      // engine consumes it only for the filtered path, but it must not error.
      const resp = br(await Batch.create('q-os')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', [1, 0, 0, 0], 2, { oversample: 3.0 }))
          .build())
        .execute(client!, db));

      expect(resp.results.r.records.length).toBe(2);
    });

    // ─── (4) Filtered ANN: and(vectorSimilarity, eq) ────────────────────────

    it('filtered ann: and(vectorSimilarity, eq) returns only filter-passing rows (filtered_vector_scan)', async () => {
      const dim = 8;
      const db = await setupDb(client!, 'vec_filtered', ['docs']);

      br(await Batch.create('mk-filt')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      // 3 clusters of 4 vectors each, tagged with group g0/g1/g2.
      const values: Array<Record<string, WireValue>> = [];
      for (let c = 0; c < 3; c += 1) {
        for (let j = 0; j < 4; j += 1) {
          const vec = j % 2 === 0 ? axisVector(dim, c) : nearAxisVector(dim, c, 0.05 + j * 0.01);
          values.push({ id: `c${c}-${j}`, embedding: vec, group: `g${c}`, cluster: c });
        }
      }
      br(await Batch.create('ins-filt')
        .add('ins', write.insert('docs', values))
        .execute(client!, db));

      // Filtered ANN: nearest to axis 1, restricted to group "g1".
      const resp = br(await Batch.create('q-filt')
        .add('r', Query.from('docs')
          .where(filter.and(
            filter.vectorSimilarity('embedding', axisVector(dim, 1), 3),
            filter.eq('group', 'g1'),
          ))
          .build())
        .execute(client!, db));

      const recs = resp.results.r.records;
      expect(recs.length).toBe(3);
      // Every returned record passed the residual filter.
      for (const r of recs) {
        expect(r.group).toBe('g1');
      }
      // The filtered-vector scan path was used.
      expect(resp.results.r.stats?.index_used).toBe('filtered_vector_scan');
    });

    it('filtered ann: empty predicate terminates with 0 records', async () => {
      const dim = 4;
      const db = await setupDb(client!, 'vec_filt_empty', ['docs']);

      br(await Batch.create('mk-empty')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      br(await Batch.create('ins-empty')
        .add('a', write.insert('docs', [{ id: 'a', embedding: [1, 0, 0, 0], group: 'x' }]))
        .add('b', write.insert('docs', [{ id: 'b', embedding: [0, 1, 0, 0], group: 'x' }]))
        .execute(client!, db));

      // Predicate group = "nonexistent" matches nothing; must return empty
      // (NOT infinite-retry, NOT hang).
      const resp = br(await Batch.create('q-empty')
        .add('r', Query.from('docs')
          .where(filter.and(
            filter.vectorSimilarity('embedding', [1, 0, 0, 0], 5),
            filter.eq('group', 'nonexistent'),
          ))
          .build())
        .execute(client!, db));

      expect(resp.results.r.records.length).toBe(0);
    });

    // ─── (5) Insert → delete → ANN ──────────────────────────────────────────

    it('delete: deleted id disappears from ANN output', async () => {
      const dim = 8;
      const db = await setupDb(client!, 'vec_del', ['docs']);

      br(await Batch.create('mk-del')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
        }))
        .execute(client!, db));

      const vecA = axisVector(dim, 0);
      const vecB = axisVector(dim, 1);
      const vecC = axisVector(dim, 2);
      br(await Batch.create('ins-del')
        .add('a', write.insert('docs', [{ id: 'a', embedding: vecA }]))
        .add('b', write.insert('docs', [{ id: 'b', embedding: vecB }]))
        .add('c', write.insert('docs', [{ id: 'c', embedding: vecC }]))
        .execute(client!, db));

      // Pre-delete: top-1 for vecA is 'a'.
      const before = br(await Batch.create('q-before')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', vecA, 1))
          .build())
        .execute(client!, db));
      expect(before.results.r.records[0].id).toBe('a');

      // Delete 'a' via the typed write.del builder.
      br(await Batch.create('del-a')
        .add('d', write.del('docs', filter.eq('id', 'a')))
        .execute(client!, db));

      // Post-delete: top-1 for vecA must NOT be 'a'.
      const after = br(await Batch.create('q-after')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', vecA, 1))
          .build())
        .execute(client!, db));
      expect(after.results.r.records.length).toBe(1);
      expect(after.results.r.records[0].id).not.toBe('a');
    });

    // ─── (6) sq8 across the fit threshold (280+ vectors, dim 16) ────────────

    it('sq8: insert 280 vectors, own vector in top-3 (soft recall)', async () => {
      const dim = 16;
      const db = await setupDb(client!, 'vec_sq8_fit', ['docs']);

      br(await Batch.create('mk-sq8-fit')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        }))
        .execute(client!, db));

      // FIT_THRESHOLD == BRUTE_FORCE_MAX == 256 in hnsw_adapter.rs — 280
      // crosses it and triggers the u8-graph fit.
      const total = 280;
      await insertClusteredBatched(client!, db, 'docs', total, dim, 32);

      // v-0 is the axis vector on axis 0; it must survive in its own top-3
      // (SQ8 dequant-rescore can reorder near-ties but the exact-match
      // vector must survive).
      const probeId = 'v-0';
      const probeVec = axisVector(dim, 0);

      const resp = br(await Batch.create('q-probe')
        .add('r', Query.from('docs')
          .where(filter.vectorSimilarity('embedding', probeVec, 3, { efSearch: 128 }))
          .build())
        .execute(client!, db));

      expect(resp.results.r.records.length).toBe(3);
      const ids = resp.results.r.records.map((r) => r.id);
      expect(ids).toContain(probeId);
    });

    it('sq8: filtered ANN works after fit (filtered_vector_scan)', async () => {
      const dim = 8;
      const db = await setupDb(client!, 'vec_sq8_filt', ['docs']);

      br(await Batch.create('mk-sq8-filt')
        .add('i', ddl.createIndex('v', 'docs', [['embedding']], {
          index_type: 'vector',
          vector_dim: dim,
          vector_metric: 'cosine',
          vector_quantization: 'sq8',
        }))
        .execute(client!, db));

      // 270 vectors, each tagged with cluster = its axis.
      await insertClusteredBatched(client!, db, 'docs', 270, dim, 32);

      const resp = br(await Batch.create('q-sq8-filt')
        .add('r', Query.from('docs')
          .where(filter.and(
            filter.vectorSimilarity('embedding', axisVector(dim, 0), 3, { efSearch: 128 }),
            filter.eq('cluster', 0),
          ))
          .build())
        .execute(client!, db));

      const recs = resp.results.r.records;
      expect(recs.length).toBe(3);
      for (const r of recs) {
        expect(r.cluster).toBe(0);
      }
      expect(resp.results.r.stats?.index_used).toBe('filtered_vector_scan');
    });
  },
);

describe('e2e-vector.test skip reason', () => {
  it('reports why the vector e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-vector] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
