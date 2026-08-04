/**
 * End-to-end HAVING + EXPLAIN dry-run tests — closes e2e gap cluster B
 * (task #975; see docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md
 * §1.4 `GroupBy.having` and §1.7 `explain`).
 *
 * Both `GroupBy.having` and `ReadQuery.explain` are first-class wire fields
 * with zero live-server coverage before this file:
 *
 * - HAVING: `crates/shamir-query-types/src/read/group_by.rs` — a filter
 *   applied AFTER grouping/aggregation.
 * - EXPLAIN: `crates/shamir-query-types/src/read/read_query.rs::explain` — a
 *   planner-only dry run that returns `QueryResult.explain: ExplainPlan`
 *   (`plan_type` + optional `index_used`/`estimated_rows`) WITHOUT
 *   materialising any rows. `PlanType` has 9 variants
 *   (`crates/shamir-query-types/src/read/query_result.rs`).
 *
 * Owns its own server (ephemeral port) — no conflict with other e2e suites.
 * Skipped automatically when the server binary is absent.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, Query, filter, select, ddl, write } from '../index.js';
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

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e HAVING + EXPLAIN dry-run (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-explain-having] connection failed. Server logs:\n' + server!.logs());
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

    // ═══════════════════════════════════════════════════════════════════
    // 1. HAVING — post-aggregation filtering
    // ═══════════════════════════════════════════════════════════════════

    describe('HAVING', () => {
      let havDb: string;

      it('setup: create db + seed group-by fixture (mirrors e2e-data.test.ts §5)', async () => {
        havDb = await setupDb(client!, 'having', ['t']);
        await seed(client!, havDb, 't', [
          { id: 'a', qty: 1, tag: 'red' },
          { id: 'b', qty: 5, tag: 'red' },
          { id: 'c', qty: 10, tag: 'blue' },
          { id: 'd', qty: 25, tag: 'blue' },
          { id: 'e', qty: 50, tag: 'green' },
          { id: 'f', qty: 100, tag: 'green' },
        ]);
      });

      it('having: sum(qty) > 30 excludes groups that pass the raw filter but fail HAVING', async () => {
        // Per-tag sums: red=1+5=6, blue=10+25=35, green=50+100=150.
        // A plain WHERE has nothing to filter pre-aggregation (all rows
        // pass); HAVING sum(qty) > 30 must exclude the "red" group (6) while
        // keeping "blue" (35) and "green" (150) — proving the filter runs
        // POST-aggregation, not on raw rows.
        //
        // HAVING references the SELECT output ALIAS as a flat top-level
        // field name (NOT a nested `['sum','qty']` path) — the executor
        // evaluates `having` against the per-group aggregate result row,
        // keyed by each SelectItem's alias (or synthetic default). See
        // `crates/shamir-engine/src/query/read/aggregate.rs::apply_group_by`
        // + `HavingView` (`shamir-types/src/record_view/record_ref.rs`).
        const resp = br(await Batch.create('hav-sum-gt')
          .add('g', Query.from('t')
            .groupBy('tag')
            .having(filter.gt('total', 30))
            .select([
              select.field('tag'),
              select.sum('qty', { alias: 'total' }),
            ])
            .orderByAsc('tag'))
          .execute(client!, havDb));
        const recs = resp.results.g.records;
        const tags = recs.map(r => r.tag);
        expect(tags).toEqual(['blue', 'green']);
        expect(tags).not.toContain('red');
        expect(recs.find(r => r.tag === 'blue')?.total).toBe(35);
        expect(recs.find(r => r.tag === 'green')?.total).toBe(150);
      });

      it('having: count(*) >= 2 combined with a WHERE pre-filter narrows both stages', async () => {
        // WHERE qty > 3 drops row 'a' (qty=1) from the "red" group, leaving
        // red with only 1 row (b, qty=5) — so HAVING count(*) >= 2 must now
        // exclude "red" (1 row survives WHERE) while blue/green (2 rows
        // each survive WHERE) still pass. Proves WHERE and HAVING compose:
        // WHERE narrows the row set BEFORE grouping, HAVING filters the
        // resulting aggregates.
        const resp = br(await Batch.create('hav-count-and-where')
          .add('g', Query.from('t')
            .where(filter.gt('qty', 3))
            .groupBy('tag')
            .having(filter.gte('n', 2))
            .select([
              select.field('tag'),
              select.countAll('n'),
            ])
            .orderByAsc('tag'))
          .execute(client!, havDb));
        const recs = resp.results.g.records;
        const tags = recs.map(r => r.tag);
        expect(tags).toEqual(['blue', 'green']);
        expect(tags).not.toContain('red');
        for (const r of recs) {
          expect(r.n).toBeGreaterThanOrEqual(2);
        }
      });

      it('having: no group satisfies the predicate — empty result set', async () => {
        const resp = br(await Batch.create('hav-none')
          .add('g', Query.from('t')
            .groupBy('tag')
            .having(filter.gt('total', 10_000))
            .select([
              select.field('tag'),
              select.sum('qty', { alias: 'total' }),
            ]))
          .execute(client!, havDb));
        expect(resp.results.g.records.length).toBe(0);
      });
    });

    // ═══════════════════════════════════════════════════════════════════
    // 2. EXPLAIN — planner dry-run, no materialisation
    // ═══════════════════════════════════════════════════════════════════

    describe('EXPLAIN', () => {
      let expDb: string;

      it('setup: create db + tables for full-scan / index / sorted-index cases', async () => {
        expDb = await setupDb(client!, 'explain', ['plain', 'indexed', 'ranged']);
        await seed(client!, expDb, 'plain', [
          { id: 'p1', name: 'alpha' },
          { id: 'p2', name: 'beta' },
          { id: 'p3', name: 'gamma' },
        ]);
      });

      it('explain: plain full-scan query -> plan_type FullScan, no materialised rows', async () => {
        const resp = br(await Batch.create('exp-fullscan')
          .add('r', Query.from('plain')
            .where(filter.eq('name', 'beta'))
            .explain())
          .execute(client!, expDb));
        const result = resp.results.r;
        // Dry-run: no rows materialised.
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        expect(result.explain!.plan_type).toBe('FullScan');
      });

      it('explain: equality lookup on a regular (hash) index -> plan_type IndexScan', async () => {
        br(await Batch.create('exp-mk-hash-idx')
          .add('i', ddl.createIndex('by_name', 'indexed', [['name']]))
          .execute(client!, expDb));
        await seed(client!, expDb, 'indexed', [
          { id: 'x1', name: 'foo' },
          { id: 'x2', name: 'bar' },
          { id: 'x3', name: 'baz' },
        ]);

        const resp = br(await Batch.create('exp-indexscan')
          .add('r', Query.from('indexed')
            .where(filter.eq('name', 'bar'))
            .explain())
          .execute(client!, expDb));
        const result = resp.results.r;
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        // Must NOT report a full scan — the whole point of the index.
        expect(result.explain!.plan_type).toBe('IndexScan');
        expect(result.explain!.plan_type).not.toBe('FullScan');
        // `index_used` reports the planner's internal index identifier
        // (e.g. "idx_3"), NOT the user-chosen `createIndex` name — assert
        // presence/sanity rather than the literal name we passed.
        expect(typeof result.explain!.index_used).toBe('string');
        expect(result.explain!.index_used!.length).toBeGreaterThan(0);
      });

      it('explain: range query on a sorted index -> plan_type SortedIndexScan', async () => {
        br(await Batch.create('exp-mk-sorted-idx')
          .add('i', ddl.createIndex('score_sorted', 'ranged', [['score']], {
            sorted: true,
          }))
          .execute(client!, expDb));
        await seed(client!, expDb, 'ranged', [
          { id: 'r1', score: 10 },
          { id: 'r2', score: 20 },
          { id: 'r3', score: 30 },
          { id: 'r4', score: 40 },
        ]);

        const resp = br(await Batch.create('exp-sortedscan')
          .add('r', Query.from('ranged')
            .where(filter.gt('score', 15))
            .explain())
          .execute(client!, expDb));
        const result = resp.results.r;
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        expect(result.explain!.plan_type).toBe('SortedIndexScan');
        expect(result.explain!.plan_type).not.toBe('FullScan');
        // Same as the hash-index case above: `index_used` is the planner's
        // internal id (e.g. "sorted_idx_4"), not the `createIndex` name.
        expect(typeof result.explain!.index_used).toBe('string');
        expect(result.explain!.index_used!.length).toBeGreaterThan(0);
      });

      it('explain: FTS query -> plan_type Index2', async () => {
        const ftsDb = await setupDb(client!, 'explain_fts', ['articles']);
        br(await Batch.create('exp-mk-fts-idx')
          .add('i', ddl.createIndex('fts_body', 'articles', [['body']], {
            index_type: 'fts',
          }))
          .execute(client!, ftsDb));
        await seed(client!, ftsDb, 'articles', [
          { id: 'a1', body: 'the quick brown fox' },
          { id: 'a2', body: 'rust systems programming' },
        ]);

        const resp = br(await Batch.create('exp-fts')
          .add('r', Query.from('articles')
            .where(filter.fts('body', 'quick fox', 'and'))
            .explain())
          .execute(client!, ftsDb));
        const result = resp.results.r;
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        expect(result.explain!.plan_type).toBe('Index2');
        expect(result.explain!.plan_type).not.toBe('FullScan');
      });

      it('explain: vector-similarity query -> plan_type Index2', async () => {
        const vecDb = await setupDb(client!, 'explain_vec', ['embeddings']);
        br(await Batch.create('exp-mk-vec-idx')
          .add('i', ddl.createIndex('vec_emb', 'embeddings', [['vec']], {
            index_type: 'vector',
            vector_dim: 4,
            vector_metric: 'cosine',
          }))
          .execute(client!, vecDb));
        await seed(client!, vecDb, 'embeddings', [
          { id: 'v1', vec: [1.0, 0.0, 0.0, 0.0] },
          { id: 'v2', vec: [0.7, 0.7, 0.0, 0.0] },
          { id: 'v3', vec: [0.0, 0.0, 0.0, 1.0] },
        ]);

        const resp = br(await Batch.create('exp-vec')
          .add('r', Query.from('embeddings')
            .where(filter.vectorSimilarity('vec', [1.0, 0.0, 0.0, 0.0], 2))
            .explain())
          .execute(client!, vecDb));
        const result = resp.results.r;
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        expect(result.explain!.plan_type).toBe('Index2');
        expect(result.explain!.plan_type).not.toBe('FullScan');
      });

      it('explain: combined with groupBy/having still returns a plan (pre-aggregation index selection)', async () => {
        // explain short-circuits before GROUP BY/HAVING is applied — the
        // reported plan reflects only the pre-aggregation WHERE-clause
        // index selection, not the grouping itself. This query has no
        // WHERE and no matching index on 'plain', so it must fall back to
        // FullScan even though groupBy/having are present on the request.
        const resp = br(await Batch.create('exp-grouped')
          .add('r', Query.from('plain')
            .groupBy('name')
            .having(filter.gt('n', 0))
            .select([select.field('name'), select.countAll('n')])
            .explain())
          .execute(client!, expDb));
        const result = resp.results.r;
        expect(result.records.length).toBe(0);
        expect(result.explain).toBeDefined();
        expect(result.explain!.plan_type).toBe('FullScan');
      });

      it('write ops are also exercised: seeded rows are queryable once explain is off', async () => {
        // Sanity check that the dry-run assertions above didn't accidentally
        // rely on rows never actually existing — re-run the same filters
        // without `explain` and confirm real rows come back.
        const resp = br(await Batch.create('exp-sanity')
          .add('r', Query.from('plain').where(filter.eq('name', 'beta')))
          .execute(client!, expDb));
        expect(resp.results.r.records.length).toBe(1);
        expect(resp.results.r.records[0].id).toBe('p2');
        // Confirm no `explain` field leaks onto a normal (non-dry-run) result.
        expect(resp.results.r.explain).toBeUndefined();
      });
    });
  },
);

describe('e2e-explain-having.test skip reason', () => {
  it('reports why the HAVING/EXPLAIN e2e tests were skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-explain-having] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
