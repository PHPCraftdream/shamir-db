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

    // ═══════════════════════════════════════════════════════════════════
    // Cluster F (#979): after_id tie-breaker + full PaginationInfo.
    //
    // `_id` (the base58 record id) is injected ONLY on the keyset-seek
    // (`Pagination::After`) response path — never on plain `.from().limit()`
    // reads. So every page below is fetched via `.after(...)` to (a) exercise
    // that path and (b) surface `_id` for the next page's tie-breaker.
    // ═══════════════════════════════════════════════════════════════════

    /**
     * Seed the tie-boundary scenario: 1 low (score 10) + 5 tied (score 50)
     * + 2 high (score 60, 70). With PAGE=3 the 5 tied rows straddle the page
     * boundary, so a value-only seek (no `after_id`) permanently drops the
     * tied rows past the first page's cutoff — the pre-#537 data-loss bug.
     * Returns the new db name.
     */
    async function seedTieBoundary(label: string): Promise<string> {
      const db = uniqueDbName(label);
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('ties', { repo: 'main' }),
        },
      });
      // Sorted index on score — required for the keyset-seek (`.after`) path.
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('score_sorted', 'ties', [['score']], {
          sorted: true,
        }))
        .execute(client!, db));
      const rows = [
        { id: 'lo', score: 10 },
        { id: 't1', score: 50 },
        { id: 't2', score: 50 },
        { id: 't3', score: 50 },
        { id: 't4', score: 50 },
        { id: 't5', score: 50 },
        { id: 'hi1', score: 60 },
        { id: 'hi2', score: 70 },
      ];
      br(await Batch.create('seed')
        .add('s', write.insert('ties', rows))
        .execute(client!, db));
      return db;
    }

    it('after_id: omitting afterId permanently drops tied rows at a page boundary (bug repro)', async () => {
      const db = await seedTieBoundary('aftid-bug');
      const PAGE = 3;

      // Page 1: seek from below all seeded scores (min=10) so the keyset
      // path runs and rows carry `_id`. The tie group (score=50) straddles
      // the PAGE=3 boundary: p1 holds the low row + 2 of the 5 tied rows.
      const p1 = br(await Batch.create('p1')
        .add('r', Query.from('ties').orderByAsc('score').after([0], PAGE))
        .execute(client!, db));
      const p1Recs = p1.results.r.records;
      expect(p1Recs.length).toBe(PAGE);
      const p1LastScore = p1Recs[p1Recs.length - 1].score as number;
      expect(p1LastScore).toBe(50); // boundary is mid-tie-group

      // Page 2 WITHOUT afterId: a bare value-only seek resumes STRICTLY PAST
      // the value 50, so every remaining tied row (score=50) is dropped.
      const p2 = br(await Batch.create('p2')
        .add('r', Query.from('ties').orderByAsc('score').after([p1LastScore], PAGE))
        .execute(client!, db));
      const p2Recs = p2.results.r.records;

      // Bug: across both pages only 2 of the 5 tied rows survive (the 2 on
      // page 1); the other 3 are lost forever. 8 rows seeded − 3 dropped = 5.
      const combined = [...p1Recs, ...p2Recs];
      const tied = combined.filter(r => r.score === 50);
      expect(tied.length).toBe(2);
      expect(combined.length).toBe(5);
      // The two non-tied high rows (60, 70) ARE recovered — only ties vanish.
      const highScores = combined
        .filter(r => (r.score as number) > 50)
        .map(r => r.score as number)
        .sort((a, b) => a - b);
      expect(highScores).toEqual([60, 70]);
    });

    it('after_id: passing the prev-page last-row _id preserves every tied row (#537 fix)', async () => {
      const db = await seedTieBoundary('aftid-fix');
      const PAGE = 3;

      // Page 1 via the keyset path → rows carry `_id`. Capture the last row's
      // `_id` to echo back as the tie-breaker.
      const p1 = br(await Batch.create('p1')
        .add('r', Query.from('ties').orderByAsc('score').after([0], PAGE))
        .execute(client!, db));
      const p1Recs = p1.results.r.records;
      expect(p1Recs.length).toBe(PAGE);
      const p1Last = p1Recs[p1Recs.length - 1];
      expect(p1Last.score).toBe(50);
      const p1LastId = p1Last._id as string;
      expect(typeof p1LastId).toBe('string'); // base58 record id

      // Page 2 WITH afterId: resume STRICTLY PAST the specific row
      // (score=50, _id=p1LastId) instead of past the bare value 50 — the
      // remaining tied rows are recovered.
      const p2 = br(await Batch.create('p2')
        .add('r', Query.from('ties').orderByAsc('score').after([50], PAGE, p1LastId))
        .execute(client!, db));
      const p2Recs = p2.results.r.records;
      expect(p2Recs.length).toBe(PAGE); // the 3 remaining tied rows

      // Across pages 1 + 2, ALL 5 tied rows are present, exactly once.
      const tied12 = [...p1Recs, ...p2Recs].filter(r => r.score === 50);
      expect(tied12.length).toBe(5);
      expect(new Set(tied12.map(r => r._id as string)).size).toBe(5);

      // Walk page 3 (still passing afterId) to prove the FULL set — including
      // the 2 high rows — is recovered with no loss and no duplicates: a
      // complete, lossless keyset traversal.
      const p2Last = p2Recs[p2Recs.length - 1];
      const p3 = br(await Batch.create('p3')
        .add('r', Query.from('ties')
          .orderByAsc('score')
          .after([p2Last.score as number], PAGE, p2Last._id as string))
        .execute(client!, db));
      const p3Recs = p3.results.r.records;
      expect(p3Recs.length).toBe(2); // hi1(60), hi2(70)

      const all = [...p1Recs, ...p2Recs, ...p3Recs];
      expect(all.length).toBe(8); // every seeded row recovered
      expect(new Set(all.map(r => r._id as string)).size).toBe(8); // no dups
    });

    it('PaginationInfo: page() + countTotal() populates all four fields on first and last page', async () => {
      const db = uniqueDbName('paginfo');
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('p', { repo: 'main' }),
        },
      });
      br(await Batch.create('mk-idx')
        .add('i', ddl.createIndex('score_sorted', 'p', [['score']], { sorted: true }))
        .execute(client!, db));
      br(await Batch.create('seed')
        .add('s', write.insert('p', [10, 20, 30, 40, 50].map(s => ({ id: `r${s}`, score: s }))))
        .execute(client!, db));

      // Page 1 of size 3 over 5 rows: total_pages = ceil(5/3) = 2; more rows
      // remain so has_next = true. current_page is populated ONLY by Page mode.
      const pg1 = br(await Batch.create('pg1')
        .add('r', Query.from('p').orderByAsc('score').page(1, 3).countTotal())
        .execute(client!, db));
      expect(pg1.results.r.records.map(x => x.score)).toEqual([10, 20, 30]);
      const info1 = pg1.results.r.pagination!;
      expect(info1.total_count).toBe(5);
      expect(info1.total_pages).toBe(2);
      expect(info1.current_page).toBe(1);
      expect(info1.has_next).toBe(true);

      // Last page (page 2): the remaining 2 rows; has_next flips to false.
      const pg2 = br(await Batch.create('pg2')
        .add('r', Query.from('p').orderByAsc('score').page(2, 3).countTotal())
        .execute(client!, db));
      expect(pg2.results.r.records.map(x => x.score)).toEqual([40, 50]);
      const info2 = pg2.results.r.pagination!;
      expect(info2.total_count).toBe(5);
      expect(info2.total_pages).toBe(2);
      expect(info2.current_page).toBe(2);
      expect(info2.has_next).toBe(false);
    });
  },
);
