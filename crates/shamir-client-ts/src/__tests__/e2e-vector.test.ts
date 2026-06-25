/**
 * End-to-end vector similarity search test.
 *
 * Creates a vector index, inserts records with vector embeddings,
 * runs a top-k similarity query, and asserts result ordering.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
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
