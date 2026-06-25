/**
 * End-to-end FTS (full-text search) test.
 *
 * Creates an FTS index on a text field, inserts documents, queries via
 * filter.fts(), and asserts matching records.
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
  'e2e FTS — createIndex(fts) + insert + fts query (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-fts] connection failed. Server logs:\n' + server!.logs());
        throw e;
      }
      db = await setupDb(client, 'fts', ['articles']);
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

    it('fts: create index + insert + fts query returns matching docs', async () => {
      // 1. Create FTS index on the "body" field.
      br(await Batch.create('mk-fts-idx')
        .add('i', ddl.createIndex('fts_body', 'articles', [['body']], {
          index_type: 'fts',
        }))
        .execute(client!, db));

      // 2. Insert documents with text bodies.
      await seed(client!, db, 'articles', [
        { id: 'a1', body: 'The quick brown fox jumps over the lazy dog' },
        { id: 'a2', body: 'Rust is a systems programming language' },
        { id: 'a3', body: 'The fox was very quick and clever' },
        { id: 'a4', body: 'Database indexing improves query performance' },
      ]);

      // 3. FTS query for "quick fox" — should match a1 and a3.
      const resp = br(await Batch.create('fts-query')
        .add('q', Query.from('articles')
          .where(filter.fts('body', 'quick fox', 'and'))
          .build())
        .execute(client!, db));

      const records = resp.results.q.records;
      const ids = records.map(r => r.id);
      expect(ids).toContain('a1');
      expect(ids).toContain('a3');
      expect(ids).not.toContain('a2');
      expect(ids).not.toContain('a4');
    });

    it('fts: "or" mode matches any token', async () => {
      // FTS query for "rust database" with OR mode — a2 and a4 should match.
      const resp = br(await Batch.create('fts-or')
        .add('q', Query.from('articles')
          .where(filter.fts('body', 'rust database', 'or'))
          .build())
        .execute(client!, db));

      const records = resp.results.q.records;
      const ids = records.map(r => r.id);
      expect(ids).toContain('a2');
      expect(ids).toContain('a4');
    });
  },
);

describe('e2e-fts.test skip reason', () => {
  it('reports why the FTS e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-fts] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
