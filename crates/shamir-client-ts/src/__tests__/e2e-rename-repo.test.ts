/**
 * End-to-end RENAME REPO test (Phase F.3).
 *
 * Creates a second repo with a table, inserts rows, renames the repo,
 * then queries the table under the new repo name — asserts ALL rows are
 * present, the old repo name no longer resolves, and a new insert under
 * the renamed repo works.
 *
 * Fast path: feed a DEBUG server binary via SHAMIR_SERVER_BIN so a slow
 * release build is not required:
 *   cargo build -p shamir-server
 *   SHAMIR_SERVER_BIN=.../target/debug/shamir-server npx vitest run \
 *     src/__tests__/e2e-rename-repo.test.ts
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
  setupDb,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// ─── test suite ──────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)(
  'e2e RENAME REPO — insert + rename + query (requires server binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error(
          '[e2e-rename-repo] connection failed. Server logs:\n' +
            server!.logs(),
        );
        throw e;
      }
      // setupDb creates a fresh db + a `main` repo with a single `items`
      // table. We add a second repo `analytics` with an `events` table
      // below.
      db = await setupDb(client, 'renamerepo', ['items']);
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

    it('renameRepo: populated table data preserved under new repo name', async () => {
      // 1. Create a second repo `analytics` with an `events` table.
      br(
        await Batch.create('mk-repo')
          .add('cr', ddl.createRepo('analytics'))
          .execute(client!, db),
      );
      br(
        await Batch.create('mk-table')
          .add('ct', ddl.createTable('events', { repo: 'analytics' }))
          .execute(client!, db),
      );

      // 2. Insert 3 rows into analytics/events.
      br(
        await Batch.create('seed')
          .add(
            's1',
            write.upsert(
              'events',
              { id: 'e1' },
              { id: 'e1', name: 'Alice' },
              { repo: 'analytics' },
            ),
          )
          .add(
            's2',
            write.upsert(
              'events',
              { id: 'e2' },
              { id: 'e2', name: 'Bob' },
              { repo: 'analytics' },
            ),
          )
          .add(
            's3',
            write.upsert(
              'events',
              { id: 'e3' },
              { id: 'e3', name: 'Carol' },
              { repo: 'analytics' },
            ),
          )
          .execute(client!, db),
      );

      // 3. Rename analytics → telemetry.
      br(
        await Batch.create('rn')
          .add('r', ddl.renameRepo('analytics', 'telemetry'))
          .execute(client!, db),
      );

      // 4. Query the table under the new repo name — ALL 3 rows present.
      const after = br(
        await Batch.create('q-after')
          .add('q', Query.withRepo('telemetry', 'events'))
          .execute(client!, db),
      );
      const recs = after.results.q.records;
      expect(recs.length).toBe(3);

      const names = recs.map((r) => r.name).sort();
      expect(names).toEqual(['Alice', 'Bob', 'Carol']);

      // 5. Old repo name must NOT resolve — query under `analytics` throws.
      await expect(
        Batch.create('q-old')
          .add('q', Query.withRepo('analytics', 'events'))
          .execute(client!, db),
      ).rejects.toThrow();

      // 6. Append a new row under the renamed repo.
      br(
        await Batch.create('append')
          .add(
            'a1',
            write.upsert(
              'events',
              { id: 'e4' },
              { id: 'e4', name: 'Dave' },
              { repo: 'telemetry' },
            ),
          )
          .execute(client!, db),
      );

      // 7. Query again — 4 rows now.
      const final = br(
        await Batch.create('q-final')
          .add('q', Query.withRepo('telemetry', 'events'))
          .execute(client!, db),
      );
      expect(final.results.q.records.length).toBe(4);
      expect(
        final.results.q.records.some((r) => r.name === 'Dave'),
      ).toBe(true);
    });

    it('renameRepo: refuses when destination repo already exists', async () => {
      // `main` exists (from setupDb) and `telemetry` exists (from the
      // previous test). Renaming main → telemetry must fail.
      await expect(
        Batch.create('rn-dup')
          .add('r', ddl.renameRepo('main', 'telemetry'))
          .execute(client!, db),
      ).rejects.toThrow();
    });
  },
);

describe('e2e-rename-repo.test skip reason', () => {
  it('reports why the rename-repo e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-rename-repo] SKIPPED — server binary not found.\n' +
          'Run `cargo build -p shamir-server` and set SHAMIR_SERVER_BIN.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
