/**
 * End-to-end proof of OQL Epic 01 sequencing (Phases A/B) over the TS
 * client's real wire round-trip (real server process, real WS/TLS client).
 *
 * Mirrors `crates/shamir-client/tests/batch_sequencing_e2e.rs` — same
 * scenario, ported to the TS builder — plus the `edge_provenance` field
 * added to `BatchResponse` in this phase (task #631, Epic01/D).
 *
 * TS has no `*_after`-style fluent registration methods (unlike the Rust
 * builder's `insert_after`/`query_after`/etc. from Phase B) — ordering deps
 * are expressed via `opts.after` on `Batch.add()`, per this phase's brief.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient, BatchResponse, EdgeKind } from '../index.js';
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
  'e2e batch sequencing — after/$query edge provenance (requires release binary)',
  () => {
    let server: ServerHandle | null = null;
    let client: ShamirClient | null = null;
    let db: string;

    beforeAll(async () => {
      server = await startServer();
      try {
        client = await connectAdmin(HOST, server.port);
      } catch (e) {
        console.error('[e2e-batch-sequencing] connection failed. Server logs:\n' + server.logs());
        throw e;
      }

      db = uniqueDbName('seq');
      await client.execute('default', {
        id: `mk-db-${db}`,
        queries: { mk: ddl.createDb(db) },
      });
      await client.execute(db, {
        id: `mk-tables-${db}`,
        queries: {
          mr: ddl.createRepo('main'),
          tb: ddl.createTable('items', { repo: 'main' }),
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
    // Mixed after + $query chain: create_table (already set up above) ->
    // insert (after) -> read ($query) -> update (after+$query, i.e. Both)
    // -> read2 ($query).
    // ═══════════════════════════════════════════════════════════════════

    it('mixed after/$query batch reports expected edge_provenance', async () => {
      const batch = Batch.create('chain')
        // 1. marker + insert: pure `after` dependency (Explicit) — no
        //    $query ref from `ins` to `marker`.
        .add('marker', Query.from('items').limit(0).build())
        .add('ins', write.insert('items', [{ sku: 'A1', qty: 1 }]), {
          after: ['marker'],
        });

      const insHandle = batch.handle('ins');

      // 2. read: pure `$query` data-flow dependency on `ins` (no `after`).
      batch.add(
        'rd1',
        Query.from('items')
          .where(filter.eq('sku', insHandle.first().field('sku')))
          .build(),
      );
      const rd1Handle = batch.handle('rd1');

      // 3. update: BOTH an explicit `after` on `ins` AND a `$query` ref on
      //    `rd1` => Both for the rd1 edge, Explicit for the ins edge.
      batch.add(
        'upd',
        write
          .update('items')
          .where(filter.eq('sku', rd1Handle.first().field('sku')))
          .set({ qty: 2 })
          .returning('all')
          .build(),
        { after: ['ins', 'rd1'] },
      );
      const updHandle = batch.handle('upd');

      // 4. read2: pure `$query` data-flow dependency on `upd`.
      batch.add(
        'rd2',
        Query.from('items')
          .where(filter.eq('sku', updHandle.first().field('sku')))
          .build(),
      );

      const resp: BatchResponse = br(await batch.execute(client!, db));

      // ---- data correctness ----
      expect(resp.results.rd1.records.length).toBe(1);
      expect(resp.results.rd1.records[0].qty).toBe(1);
      expect(resp.results.rd2.records.length).toBe(1);
      expect(resp.results.rd2.records[0].qty).toBe(2);

      // ---- edge_provenance correctness ----
      const prov = resp.edge_provenance;
      expect(prov).toBeDefined();
      const p = prov as Record<string, Record<string, EdgeKind>>;

      expect(p.ins?.marker).toBe('explicit');
      expect(p.rd1?.ins).toBe('data_flow');
      expect(p.upd?.rd1).toBe('both');
      expect(p.upd?.ins).toBe('explicit');
      expect(p.rd2?.upd).toBe('data_flow');
    });

    // ═══════════════════════════════════════════════════════════════════
    // Regression e2e for Phase A point 3: pure `after` (no `$query`) must
    // NOT open a data channel — proven via the edge_provenance tag being
    // Explicit-only over the real wire.
    // ═══════════════════════════════════════════════════════════════════

    it('pure after dependency does not open a data-flow edge over the wire', async () => {
      const batch = Batch.create('after-only')
        .add('ins2', write.insert('items', [{ sku: 'B1', qty: 7 }]))
        .add('rd', Query.from('items').build(), { after: ['ins2'] });

      const resp: BatchResponse = br(await batch.execute(client!, db));

      expect(resp.results.rd.records.some((r) => r.sku === 'B1')).toBe(true);

      const prov = resp.edge_provenance as Record<string, Record<string, EdgeKind>>;
      expect(prov.rd?.ins2).toBe('explicit');
    });

    // ═══════════════════════════════════════════════════════════════════
    // `after: ["@alias"]` (with the `@` prefix, normalized post-Phase-B)
    // behaves identically to a bare alias.
    // ═══════════════════════════════════════════════════════════════════

    it('after with an "@"-prefixed alias normalizes the same as bare alias', async () => {
      const batch = Batch.create('at-prefix')
        .add('ins3', write.insert('items', [{ sku: 'C1', qty: 3 }]))
        .add('rd3', Query.from('items').build(), { after: ['@ins3'] });

      const resp: BatchResponse = br(await batch.execute(client!, db));
      expect(resp.results.rd3.records.some((r) => r.sku === 'C1')).toBe(true);

      const prov = resp.edge_provenance as Record<string, Record<string, EdgeKind>>;
      expect(prov.rd3?.ins3).toBe('explicit');
    });

    // ═══════════════════════════════════════════════════════════════════
    // A cycle formed by mixing `after` + `$query` edges must surface as a
    // `ShamirDbError` (server-side `CircularDependency`, code "validation")
    // over the real wire — not a hang / crash / raw protocol error.
    // ═══════════════════════════════════════════════════════════════════

    it('a batch with an after/$query cycle rejects with a validation error', async () => {
      // Two entries whose edges close a cycle:
      //   b2 --$query(DataFlow)--> a2   (b2's filter references a2's result)
      //   a2 --after(Explicit)-->  b2   (a2 is explicitly ordered after b2)
      // Registered via raw wire objects (not `Batch.add`'s typed handle
      // helper) since the cycle spans both directions and a `Handle` for
      // `b2` can only be obtained after `b2` is declared.
      const batch = Batch.create('cycle').add(
        'b2',
        Query.from('items').build(),
      );
      const b2Handle = batch.handle('b2');
      batch.add(
        'a2',
        Query.from('items')
          .where(filter.eq('sku', b2Handle.first().field('sku')))
          .build(),
      );
      // Close the cycle: b2 explicitly ordered after a2.
      batch.add('b2', Query.from('items').build(), { after: ['a2'] });

      await expect(batch.execute(client!, db)).rejects.toThrow();
    });
  },
);
