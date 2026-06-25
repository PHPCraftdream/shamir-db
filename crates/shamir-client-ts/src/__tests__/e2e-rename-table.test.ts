/**
 * End-to-end RENAME TABLE test (Phase F.2 — populated-table rename).
 *
 * Creates a table, inserts several rows, renames the table, then queries
 * the new table — asserts ALL rows are present, the old name no longer
 * resolves, and new inserts into the renamed table work.
 *
 * Requires the release server binary:
 *   cargo build --release -p shamir-server
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
  'e2e RENAME TABLE — insert + rename + query (requires release binary)',
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
          '[e2e-rename-table] connection failed. Server logs:\n' +
            server!.logs(),
        );
        throw e;
      }
      db = await setupDb(client, 'renametable', ['users']);
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

    it('renameTable: populated table data preserved under new name', async () => {
      // 1. Insert 3 rows into `users`.
      br(
        await Batch.create('seed')
          .add(
            's1',
            write.upsert('users', { id: 'u1' }, {
              id: 'u1',
              name: 'Alice',
              email: 'alice@example.com',
            }),
          )
          .add(
            's2',
            write.upsert('users', { id: 'u2' }, {
              id: 'u2',
              name: 'Bob',
              email: 'bob@example.com',
            }),
          )
          .add(
            's3',
            write.upsert('users', { id: 'u3' }, {
              id: 'u3',
              name: 'Carol',
              email: 'carol@example.com',
            }),
          )
          .execute(client!, db),
      );

      // 2. Rename users → people.
      br(
        await Batch.create('rn')
          .add('r', ddl.renameTable('users', 'people', { repo: 'main' }))
          .execute(client!, db),
      );

      // 3. Query the renamed table — ALL 3 rows must be present.
      const after = br(
        await Batch.create('q-after')
          .add('q', Query.from('people'))
          .execute(client!, db),
      );
      const recs = after.results.q.records;
      expect(recs.length).toBe(3);

      const names = recs.map((r) => r.name).sort();
      expect(names).toEqual(['Alice', 'Bob', 'Carol']);

      // 4. Old name must NOT resolve — the client throws on query failure.
      await expect(
        Batch.create('q-old')
          .add('q', Query.from('users'))
          .execute(client!, db),
      ).rejects.toThrow();

      // 5. Append a new row into the renamed table.
      br(
        await Batch.create('append')
          .add(
            'a1',
            write.upsert('people', { id: 'u4' }, {
              id: 'u4',
              name: 'Dave',
              email: 'dave@example.com',
            }),
          )
          .execute(client!, db),
      );

      // 6. Query again — 4 rows now.
      const final = br(
        await Batch.create('q-final')
          .add('q', Query.from('people'))
          .execute(client!, db),
      );
      expect(final.results.q.records.length).toBe(4);
      expect(
        final.results.q.records.some((r) => r.name === 'Dave'),
      ).toBe(true);
    });
  },
);

describe('e2e-rename-table.test skip reason', () => {
  it('reports why the rename-table e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-rename-table] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
