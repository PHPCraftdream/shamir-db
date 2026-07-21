/**
 * FG-2 e2e: `with_version` / `expected_version` optimistic-concurrency (CAS)
 * contour through the TS SDK against a REAL server process.
 *
 * Mirrors `crates/shamir-server/tests/version_cas_e2e.rs` (the Rust e2e
 * twin) and `crates/shamir-engine/src/table/tests/version_cas_tests.rs`'s
 * concurrent scenario, but drives it through the TS `Query.withVersion()` /
 * `write.update(...).expectedVersion(v)` builders and `isVersionConflict`.
 *
 * Follows the `describe.skipIf(!SERVER_AVAILABLE)` self-skip convention
 * from `e2e-harness.ts` (RI-5) — this suite silently no-ops when the
 * release `shamir-server` binary hasn't been built.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse } from '../index.js';
import { Query, Batch, filter, write, ddl } from '../index.js';
import { isVersionConflict, ShamirDbError } from '../index.js';
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
  'e2e with_version / expected_version CAS (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-version-cas] connection failed. Server logs:\n' + server.logs());
        throw e;
      }

      db = uniqueDbName('vcas');
      await client.execute('default', {
        id: `mk-db-${db}`,
        queries: { mk: ddl.createDb(db) },
      });
      await client.execute(db, {
        id: `mk-tables-${db}`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('kv', { repo: 'main' }),
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

    async function readVersion(id: string): Promise<number> {
      const resp: BatchResponse = br(
        await client!.execute(db, {
          id: `read-${id}-${Date.now()}`,
          queries: {
            g: Query.from('kv').where(filter.eq('id', id)).withVersion().build(),
          },
        }),
      );
      const result = resp.results.g as unknown as { versions?: number[]; records: unknown[] };
      expect(result.records).toHaveLength(1);
      expect(result.versions).toBeDefined();
      return result.versions![0];
    }

    it('withVersion() read + expectedVersion() write round-trip over the real wire', async () => {
      await client!.execute(db, {
        id: `seed-${db}-1`,
        queries: { p: write.upsert('kv', { id: 'row1' }, { id: 'row1', val: 1 }) },
      });

      const v0 = await readVersion('row1');

      // Matching expected_version succeeds.
      await client!.execute(db, {
        id: `up-${db}-1`,
        queries: {
          u: write.update('kv').where(filter.eq('id', 'row1')).set({ val: 2 }).expectedVersion(v0).build(),
        },
      });

      const v1 = await readVersion('row1');
      expect(v1).toBeGreaterThan(v0);

      // Stale expected_version is rejected with the typed version_conflict code.
      let caught: unknown;
      try {
        await client!.execute(db, {
          id: `up-${db}-2`,
          queries: {
            u2: write
              .update('kv')
              .where(filter.eq('id', 'row1'))
              .set({ val: 999 })
              .expectedVersion(v0)
              .build(),
          },
        });
      } catch (e) {
        caught = e;
      }
      expect(caught).toBeInstanceOf(ShamirDbError);
      expect((caught as ShamirDbError).code).toBe('version_conflict');
      expect(isVersionConflict(caught)).toBe(true);

      // Row unchanged after the rejected attempt.
      const verify: BatchResponse = br(
        await client!.execute(db, {
          id: `verify-${db}-1`,
          queries: { g: Query.from('kv').where(filter.eq('id', 'row1')).build() },
        }),
      );
      const rows = verify.results.g.records as Array<{ val: number }>;
      expect(rows).toHaveLength(1);
      expect(rows[0].val).toBe(2);
    });

    it('CONCURRENT CAS: two real racing writers, exactly one wins, retry with fresh version succeeds', async () => {
      await client!.execute(db, {
        id: `seed-${db}-2`,
        queries: { p: write.upsert('kv', { id: 'counter' }, { id: 'counter', val: 0 }) },
      });

      const v0 = await readVersion('counter');

      // Two real concurrent client requests racing the SAME expected_version,
      // launched together via Promise.allSettled (genuinely concurrent, not
      // sequential awaits).
      const attempt = (val: number) =>
        client!.execute(db, {
          id: `race-${val}-${Date.now()}`,
          queries: {
            u: write
              .update('kv')
              .where(filter.eq('id', 'counter'))
              .set({ val })
              .expectedVersion(v0)
              .build(),
          },
        });

      const [resA, resB] = await Promise.allSettled([attempt(100), attempt(200)]);

      const aOk = resA.status === 'fulfilled';
      const bOk = resB.status === 'fulfilled';
      const aConflict =
        resA.status === 'rejected' && isVersionConflict(resA.reason);
      const bConflict =
        resB.status === 'rejected' && isVersionConflict(resB.reason);

      if (!((aOk && bConflict) || (bOk && aConflict))) {
        throw new Error(
          `expected exactly one success and one version_conflict, got: ` +
            `aOk=${aOk} bOk=${bOk} aConflict=${aConflict} bConflict=${bConflict}`,
        );
      }
      expect((aOk && bConflict) || (bOk && aConflict)).toBe(true);

      // Retry with the fresh version must succeed.
      const v1 = await readVersion('counter');
      expect(v1).toBeGreaterThan(v0);

      await client!.execute(db, {
        id: `retry-${db}`,
        queries: {
          u: write
            .update('kv')
            .where(filter.eq('id', 'counter'))
            .set({ val: 999 })
            .expectedVersion(v1)
            .build(),
        },
      });
    });

    it('reports why the e2e-version-cas test was skipped', () => {
      if (!SERVER_AVAILABLE) {
        console.warn(
          '[e2e-version-cas] SKIPPED: release shamir-server binary not found. ' +
            'Build it with: cargo build --release -p shamir-server',
        );
      }
      expect(true).toBe(true);
    });
  },
);
