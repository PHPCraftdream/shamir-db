/**
 * End-to-end proof of OQL Epic 02 `$cond`/`switchCase` value evaluation
 * (Phases A/B/C) over the TS client's real wire round-trip (real server
 * process, real WS/TLS client).
 *
 * Mirrors `crates/shamir-client/tests/batch_cond_e2e.rs` — same scenarios,
 * ported to the TS builder (`filter.cond`/`filter.switchCase` from Phase B).
 *
 * Per the correction discovered in Epic02/B (task #641): `$cond` does NOT
 * compose into write SET-values today (`UpdateOp.set`/`SetOp.value` are
 * typed as `QueryValue`, which structurally cannot hold `FilterValue::Cond`).
 * That gap is out of scope here — every scenario below uses
 * `filter.cond`/`filter.switchCase` in a WHERE-filter comparison value,
 * which is the fully-supported path today.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse } from '../index.js';
import { Query, Batch, filter, write, ddl } from '../index.js';
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
  'e2e $cond/switchCase value evaluation (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-cond] connection failed. Server logs:\n' + server.logs());
        throw e;
      }

      db = uniqueDbName('cond');
      await client.execute('default', {
        id: `mk-db-${db}`,
        queries: { mk: ddl.createDb(db) },
      });
      await client.execute(db, {
        id: `mk-tables-${db}`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('users', { repo: 'main' }),
        },
      });
    }, 60_000);

    afterAll(async () => {
      if (client) {
        try {
          await client.close();
        } catch {
          /* ok */
        }
        client = null;
      }
      if (server) {
        await server.stop();
        server = null;
      }
    }, 15_000);

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 1: switchCase in a WHERE-filter classifies records by score
    // (vip/regular/newbie), same canonical example as switchCase's docstring.
    // ═══════════════════════════════════════════════════════════════════

    it('switchCase WHERE filter classifies records over the real wire', async () => {
      await client!.execute(db, {
        id: `seed-${db}-1`,
        queries: {
          ins: write.insert('users', [
            { name: 'alice', scenario: 'sc1', score: 120, tier: 'vip' },
            { name: 'bob', scenario: 'sc1', score: 75, tier: 'regular' },
            { name: 'carol', scenario: 'sc1', score: 10, tier: 'newbie' },
            // dave is deliberately mis-tagged: score=120 classifies as
            // "vip" via switchCase, but his stored tier is "regular" — he
            // must NOT match, proving the engine evaluates per-record
            // rather than short-circuiting to a constant/default.
            { name: 'dave', scenario: 'sc1', score: 120, tier: 'regular' },
          ]),
        },
      });

      const tierSwitchCase = filter.switchCase(
        [
          [filter.gte('score', 100), 'vip'],
          [filter.gte('score', 50), 'regular'],
        ],
        'newbie',
      );

      const resp: BatchResponse = br(
        await client!.execute(db, {
          id: `rd-${db}-1`,
          queries: {
            rd: Query.from('users')
              .where(filter.and(filter.eq('scenario', 'sc1'), filter.eq('tier', tierSwitchCase)))
              .build(),
          },
        }),
      );

      const names = (resp.results.rd.records as Array<{ name: string }>)
        .map((r) => r.name)
        .sort();

      expect(names).toEqual(['alice', 'bob', 'carol']);
    });

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 2: a nested $cond (3 levels deep, hand-nested via
    // filter.cond rather than switchCase sugar) evaluated over the real
    // wire — proves the engine recurses through all levels.
    // ═══════════════════════════════════════════════════════════════════

    it('nested 3-level $cond evaluates correctly over the real wire', async () => {
      await client!.execute(db, {
        id: `seed-${db}-2`,
        queries: {
          ins: write.insert('users', [
            { name: 'eve', scenario: 'sc2', score: 200, tier: 'vip' },
            { name: 'frank', scenario: 'sc2', score: 60, tier: 'regular' },
            { name: 'grace', scenario: 'sc2', score: 5, tier: 'newbie' },
          ]),
        },
      });

      const nested = filter.cond(
        filter.gte('score', 100),
        'vip',
        filter.cond(
          filter.gte('score', 50),
          'regular',
          filter.cond(filter.gte('score', 1), 'newbie', 'unranked'),
        ),
      );

      const resp: BatchResponse = br(
        await client!.execute(db, {
          id: `rd-${db}-2`,
          queries: {
            rd: Query.from('users')
              .where(filter.and(filter.eq('scenario', 'sc2'), filter.eq('tier', nested)))
              .build(),
          },
        }),
      );

      const names = (resp.results.rd.records as Array<{ name: string }>)
        .map((r) => r.name)
        .sort();

      expect(names).toEqual(['eve', 'frank', 'grace']);
    });

    // ═══════════════════════════════════════════════════════════════════
    // Scenario 3: a $cond branch referencing a prior query's result
    // (cross-query conditional value) — a real wire round-trip.
    //
    // Regression test for bug #642 (now fixed):
    // `BatchPlanner::extract_deps_from_filter_value`
    // (crates/shamir-query-types/src/batch/planner.rs:618-648) recurses into
    // `FilterValue::Cond`/`Expr`/`FnCall` (in addition to `Array` and
    // `QueryRef`), so a `$query` ref nested inside a `$cond` branch used as a
    // WHERE-filter comparison value is correctly extracted as a dependency
    // and the batch planner adds a `DataFlow`/`Both` edge for the dependent
    // op. `query_runner::build_resolved_refs`
    // (crates/shamir-engine/src/query/batch/query_runner.rs) then copies the
    // dependency's result into the dependent op's FilterContext, so the
    // `$query`-ref branch of `$cond` resolves correctly instead of silently
    // missing.
    //
    // Mirrors the Rust twin
    // `crates/shamir-client/tests/batch_cond_e2e.rs::cond_branch_referencing_prior_query_result_over_real_wire`.
    // ═══════════════════════════════════════════════════════════════════

    it('$cond branch referencing a prior query result resolves correctly', async () => {
      await client!.execute(db, {
        id: `seed-${db}-3`,
        queries: {
          ins: write.insert('users', [
            { name: 'heidi', scenario: 'sc3', score: 100, tier: 'vip' },
            { name: 'ivan', scenario: 'sc3', score: 30, tier: 'newbie' },
          ]),
        },
      });

      const batch = Batch.create('cross').add(
        'threshold_lookup',
        Query.from('users').where(filter.eq('name', 'heidi')).build(),
      );
      const threshold = batch.handle('threshold_lookup');

      // WHERE tier == cond(score >= 100, <tier of heidi row>, "newbie")
      // For heidi (score=100, tier="vip"): evaluates to threshold_lookup's
      // tier ("vip"), matching her own tier.
      // For ivan (score=30, tier="newbie"): evaluates to the literal
      // "newbie", which equals his own tier, so he matches too.
      const crossCond = filter.cond(
        filter.gte('score', 100),
        threshold.first().field('tier'),
        'newbie',
      );

      // Explicit `after` is redundant now (the $query ref inside $cond is
      // correctly auto-detected as a DataFlow dependency, #642) but kept for
      // clarity/defense-in-depth ordering.
      batch.add(
        'rd',
        Query.from('users')
          .where(filter.and(filter.eq('scenario', 'sc3'), filter.eq('tier', crossCond)))
          .build(),
        { after: ['threshold_lookup'] },
      );

      const resp: BatchResponse = br(await batch.execute(client!, db));

      const names = (resp.results.rd.records as Array<{ name: string }>)
        .map((r) => r.name)
        .sort();

      // Both match now that bug #642 is fixed: heidi's $cond branch
      // correctly resolves threshold_lookup's tier ("vip") via the
      // now-detected DataFlow dependency; ivan matches via the plain
      // literal "newbie" branch.
      expect(names).toEqual(['heidi', 'ivan']);
    });
  },
);
