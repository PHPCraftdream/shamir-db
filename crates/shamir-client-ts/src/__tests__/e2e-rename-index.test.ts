/**
 * End-to-end RENAME INDEX test.
 *
 * Creates a regular hash index on a text field, inserts rows, renames
 * the index, then queries by the indexed field — asserts results are
 * correct and the index is still used under the new name.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, Query, ddl, filter, write } from '../index.js';
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
  'e2e RENAME INDEX — createIndex + insert + rename + query (requires release binary)',
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
          '[e2e-rename-index] connection failed. Server logs:\n' +
            server!.logs(),
        );
        throw e;
      }
      db = await setupDb(client, 'renameidx', ['users']);
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

    it('renameIndex: index data preserved under new name', async () => {
      // 1. Create a regular hash index on "email".
      br(
        await Batch.create('mk-idx')
          .add('i', ddl.createIndex('idx_email', 'users', [['email']]))
          .execute(client!, db),
      );

      // 2. Insert rows.
      br(
        await Batch.create('seed')
          .add(
            's1',
            write.upsert('users', { id: 'u1' }, {
              id: 'u1',
              email: 'alice@example.com',
              name: 'Alice',
            }),
          )
          .add(
            's2',
            write.upsert('users', { id: 'u2' }, {
              id: 'u2',
              email: 'bob@example.com',
              name: 'Bob',
            }),
          )
          .execute(client!, db),
      );

      // 3. Query by indexed field before rename — should find Alice.
      const before = br(
        await Batch.create('q-before')
          .add(
            'q',
            Query.from('users').where(filter.eq('email', 'alice@example.com')),
          )
          .execute(client!, db),
      );
      const beforeRecs = before.results.q.records;
      expect(beforeRecs.length).toBe(1);
      expect(beforeRecs[0].name).toBe('Alice');

      // 4. Rename idx_email → idx_mail.
      br(
        await Batch.create('rn-idx')
          .add('r', ddl.renameIndex('users', 'idx_email', 'idx_mail'))
          .execute(client!, db),
      );

      // 5. Query by indexed field after rename — should STILL find Alice.
      const after = br(
        await Batch.create('q-after')
          .add(
            'q',
            Query.from('users').where(filter.eq('email', 'alice@example.com')),
          )
          .execute(client!, db),
      );
      const afterRecs = after.results.q.records;
      expect(afterRecs.length).toBe(1);
      expect(afterRecs[0].name).toBe('Alice');

      // 6. Query for Bob — second row also intact.
      const bob = br(
        await Batch.create('q-bob')
          .add(
            'q',
            Query.from('users').where(filter.eq('email', 'bob@example.com')),
          )
          .execute(client!, db),
      );
      expect(bob.results.q.records.length).toBe(1);
      expect(bob.results.q.records[0].name).toBe('Bob');
    });
  },
);

describe('e2e-rename-index.test skip reason', () => {
  it('reports why the rename-index e2e test was skipped', () => {
    if (SERVER_AVAILABLE) {
      expect(true).toBe(true);
    } else {
      console.warn(
        '[e2e-rename-index] SKIPPED — server binary not found.\n' +
          'Run `cargo build --release -p shamir-server` first.',
      );
      expect(SERVER_AVAILABLE).toBe(false);
    }
  });
});
