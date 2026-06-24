/**
 * End-to-end test for Phase D.1 — ON DELETE RESTRICT.
 *
 * Parent + child tables; child schema declares
 * `.foreignKey('parent', 'id', { onDelete: 'restrict' })` with a required
 * index on the parent table's `id` field.  Exercises:
 *
 * 1. Delete parent with a referencing child → rejected (fk_restrict).
 * 2. Delete child first, then delete parent → succeeds.
 *
 * Own startServer (ephemeral port) — no conflict with other e2e suites.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';

import type { ShamirClient } from '../index.js';
import { Batch, ddl, write, filter } from '../index.js';
import {
  SERVER_AVAILABLE,
  HOST,
  startServer,
  connectAdmin,
  br,
  uniqueDbName,
  setupDb,
} from './e2e-harness.js';
import type { ServerHandle } from './e2e-harness.js';

// ── suite ───────────────────────────────────────────────────────────────

describe.skipIf(!SERVER_AVAILABLE)('Phase D.1 — ON DELETE RESTRICT', () => {
  let srv: ServerHandle;
  let client: ShamirClient | null = null;
  let db: string;

  beforeAll(async () => {
    srv = await startServer();
    client = await connectAdmin(HOST, srv.port);

    db = await setupDb(client, 'fk_ondelete', ['parent', 'child']);

    // Create index on parent.id (required for FK DDL gate).
    br(
      await Batch.create('idx')
        .add('idx', ddl.createIndex('parent_id_idx', 'parent', [['id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Create index on child.parent_id (for efficient restrict lookup).
    br(
      await Batch.create('idx2')
        .add('idx', ddl.createIndex('child_parent_id_idx', 'child', [['parent_id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Set schema on child: parent_id is FK to parent.id with on_delete=restrict.
    br(
      await Batch.create('schema')
        .add(
          's',
          ddl.setTableSchema('child', [
            ddl
              .field(['parent_id'])
              .int()
              .required()
              .foreignKey('parent', 'id', { onDelete: 'restrict' })
              .build(),
            ddl.field(['label']).string().required().build(),
          ]),
        )
        .execute(client, db),
    );
  }, 30_000);

  afterAll(async () => {
    client?.close();
    await srv?.stop();
  });

  // ────────────────────────────────────────────────────────────────────

  it('reject parent delete when child references it (fk_restrict)', async () => {
    // Insert parent row.
    br(
      await Batch.create('ins-parent')
        .add('p', write.insert('parent', { id: 1, name: 'Alice' }))
        .execute(client!, db),
    );

    // Insert child referencing parent.
    br(
      await Batch.create('ins-child')
        .add('c', write.insert('child', { parent_id: 1, label: 'x' }))
        .transactional()
        .execute(client!, db),
    );

    // Try to delete parent → expect fk_restrict error.
    try {
      br(
        await Batch.create('del-parent')
          .add('d', write.del('parent', filter.eq('id', 1)))
          .execute(client!, db),
      );
      expect.fail('should have thrown fk_restrict');
    } catch (e: unknown) {
      expect((e as Error).message).toContain('fk_restrict');
    }
  });

  it('delete child first, then parent succeeds', async () => {
    // Insert parent row (use unique id to avoid collision with previous test).
    br(
      await Batch.create('ins-parent2')
        .add('p', write.insert('parent', { id: 2, name: 'Bob' }))
        .execute(client!, db),
    );

    // Insert child referencing parent.
    br(
      await Batch.create('ins-child2')
        .add('c', write.insert('child', { parent_id: 2, label: 'y' }))
        .transactional()
        .execute(client!, db),
    );

    // Delete child first.
    br(
      await Batch.create('del-child')
        .add('d', write.del('child', filter.eq('parent_id', 2)))
        .execute(client!, db),
    );

    // Delete parent → should succeed now.
    br(
      await Batch.create('del-parent2')
        .add('d', write.del('parent', filter.eq('id', 2)))
        .execute(client!, db),
    );
  });
});
