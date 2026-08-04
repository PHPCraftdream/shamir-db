/**
 * End-to-end NULLS ordering — exercises the in-memory (brute-force) sort
 * path's `NullsOrder` handling
 * (`crates/shamir-engine/src/query/read/order.rs::compare_qv_sort_keys`).
 *
 * SQL-standard defaults when `nulls` is unset (order.rs ~line 457):
 *   ASC  → nulls LAST
 *   DESC → nulls FIRST
 * An explicit `nulls` argument (`orderByAsc(field, 'first'|'last')` /
 * `orderByDesc(field, 'first'|'last')`) overrides that default in EITHER
 * direction.
 *
 * This is a PLAIN `orderBy` with NO sorted index, so the brute-force
 * comparator is the code under test (an index seek would be a different
 * path). Owns its own server; skipped when the binary is absent.
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
  'e2e NULLS ordering (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-nulls] connection failed. Server logs:\n' + server.logs());
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

    // Seed: one NULL `score` row (b) among non-null rows. A single null row
    // avoids any null-vs-null tie-order ambiguity. The `id` field is just a
    // stable label used to assert position; `score` is the ORDER BY column.
    // NO sorted index → forces the in-memory sort path under test.
    const SEEDED = [
      { id: 'a', score: 10 },
      { id: 'b', score: null },
      { id: 'c', score: 30 },
      { id: 'e', score: 20 },
    ];

    it('ASC default places nulls LAST (SQL standard)', async () => {
      const db = uniqueDbName('nulls-asc-def');
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('n', { repo: 'main' }),
        },
      });
      br(await Batch.create('seed')
        .add('s', write.insert('n', SEEDED))
        .execute(client!, db));

      const resp = br(await Batch.create('q')
        .add('r', Query.from('n').orderByAsc('score'))
        .execute(client!, db));
      // [10, 20, 30, null] → null row 'b' sorts LAST.
      expect(resp.results.r.records.map(x => x.id)).toEqual(['a', 'e', 'c', 'b']);
    });

    it('ASC + explicit nullsFirst overrides the default (null FIRST)', async () => {
      const db = uniqueDbName('nulls-asc-first');
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('n', { repo: 'main' }),
        },
      });
      br(await Batch.create('seed')
        .add('s', write.insert('n', SEEDED))
        .execute(client!, db));

      const resp = br(await Batch.create('q')
        .add('r', Query.from('n').orderByAsc('score', 'first'))
        .execute(client!, db));
      // [null, 10, 20, 30] → null row 'b' sorts FIRST.
      expect(resp.results.r.records.map(x => x.id)).toEqual(['b', 'a', 'e', 'c']);
    });

    it('DESC default places nulls FIRST (SQL standard)', async () => {
      const db = uniqueDbName('nulls-desc-def');
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('n', { repo: 'main' }),
        },
      });
      br(await Batch.create('seed')
        .add('s', write.insert('n', SEEDED))
        .execute(client!, db));

      const resp = br(await Batch.create('q')
        .add('r', Query.from('n').orderByDesc('score'))
        .execute(client!, db));
      // [null, 30, 20, 10] → null row 'b' sorts FIRST.
      expect(resp.results.r.records.map(x => x.id)).toEqual(['b', 'c', 'e', 'a']);
    });

    it('DESC + explicit nullsLast overrides the default (null LAST)', async () => {
      const db = uniqueDbName('nulls-desc-last');
      await client!.execute('default', {
        id: `setup-${db}-db`,
        queries: { mk: ddl.createDb(db) },
      });
      await client!.execute(db, {
        id: `setup-${db}-table`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('n', { repo: 'main' }),
        },
      });
      br(await Batch.create('seed')
        .add('s', write.insert('n', SEEDED))
        .execute(client!, db));

      const resp = br(await Batch.create('q')
        .add('r', Query.from('n').orderByDesc('score', 'last'))
        .execute(client!, db));
      // [30, 20, 10, null] → null row 'b' sorts LAST.
      expect(resp.results.r.records.map(x => x.id)).toEqual(['c', 'e', 'a', 'b']);
    });
  },
);
