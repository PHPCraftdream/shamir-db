/**
 * End-to-end keyset (seek) pagination test — exercises the `After` pagination
 * mode against a real shamir-server with a sorted index.
 *
 * Pattern: create a table + SORTED index on `score`, seed 8 rows with
 * increasing scores, fetch page 1 via `.limit(3)`, then page 2 via
 * `.after([lastScore], 3)` and assert strict-after / no-overlap / correct
 * order.
 *
 * Owns its own server (ephemeral port) — no conflict with other e2e suites.
 * Skipped automatically when the server binary is absent.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, Query, ddl, write } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e keyset (seek) pagination (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-keyset] connection failed. Server logs:\n' + server.logs());
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

    // 8 rows with distinct increasing scores.
    const SCORES = [10, 20, 30, 40, 50, 60, 70, 80];
    const PAGE = 3;

    it('keyset: page 1 via limit, page 2 via after — strict-after, no overlap', async () => {
      const db = uniqueDbName('keyset');

      // ── setup: db + repo + table ────────────────────────────────────
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('users', { repo: 'main' }),
        },
      });

      // ── sorted index on score (required for the seek path) ─────────
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('score_sorted', 'users', [['score']], {
          sorted: true,
        }))
        .execute(client!, db));

      // ── seed 8 rows ────────────────────────────────────────────────
      const rows = SCORES.map((score, i) => ({
        id: `u${i + 1}`,
        score,
      }));
      br(await Batch.create('seed')
        .add('s', write.insert('users', rows))
        .execute(client!, db));

      // ── page 1: first 3 by score asc ───────────────────────────────
      const p1 = br(await Batch.create('p1')
        .add('r', Query.from('users').orderByAsc('score').limit(PAGE))
        .execute(client!, db));
      const p1Recs = p1.results.r.records;
      expect(p1Recs.length).toBe(PAGE);
      const p1Scores = p1Recs.map(r => r.score as number);
      expect(p1Scores).toEqual([10, 20, 30]);

      // ── page 2: seek after the last row's score ────────────────────
      const lastScore = p1Scores[p1Scores.length - 1];
      const p2 = br(await Batch.create('p2')
        .add('r', Query.from('users')
          .orderByAsc('score')
          .after([lastScore], PAGE))
        .execute(client!, db));
      const p2Recs = p2.results.r.records;
      expect(p2Recs.length).toBe(PAGE);
      const p2Scores = p2Recs.map(r => r.score as number);

      // Strictly after page 1's last score, contiguous, correct order.
      expect(p2Scores).toEqual([40, 50, 60]);
      for (const s of p2Scores) {
        expect(s).toBeGreaterThan(lastScore);
      }
      // No overlap with page 1.
      const overlap = p1Scores.filter(s => p2Scores.includes(s));
      expect(overlap).toEqual([]);
    });
  },
);
