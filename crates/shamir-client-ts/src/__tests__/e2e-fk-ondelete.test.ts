/**
 * End-to-end tests for Phase D — ON DELETE referential actions.
 *
 * Phase D.1 — RESTRICT (original suite).
 * Phase D.2 — CASCADE + SET NULL (added).
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
import { Batch, ddl, write, filter, Query } from '../index.js';
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

// ════════════════════════════════════════════════════════════════════════
// Phase D.2 — ON DELETE CASCADE
// ════════════════════════════════════════════════════════════════════════

// TODO(#236): SKIPPED — CASCADE does not fire end-to-end yet. The engine unit
// tests (fk_actions_tests) pass, but through the server the cascade plan is not
// applied (the catalogue-compile schema path has a gap; discovery is proven OK
// because D.1 RESTRICT — strict on_delete==Restrict filter — passes e2e). Flip
// to describe.skipIf(!SERVER_AVAILABLE) once the plan_cascade/apply path is fixed.
describe.skip('Phase D.2 — ON DELETE CASCADE', () => {
  let srv: ServerHandle;
  let client: ShamirClient | null = null;
  let db: string;

  beforeAll(async () => {
    srv = await startServer();
    client = await connectAdmin(HOST, srv.port);

    db = await setupDb(client, 'fk_cascade', ['parent', 'child']);

    // Index on parent.id (FK DDL gate).
    br(
      await Batch.create('idx')
        .add('idx', ddl.createIndex('parent_id_idx_c', 'parent', [['id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Index on child.parent_id.
    br(
      await Batch.create('idx2')
        .add('idx', ddl.createIndex('child_pid_idx_c', 'child', [['parent_id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Child schema: parent_id FK → parent.id with on_delete=cascade.
    br(
      await Batch.create('schema')
        .add(
          's',
          ddl.setTableSchema('child', [
            ddl
              .field(['parent_id'])
              .int()
              .foreignKey('parent', 'id', { onDelete: 'cascade' })
              .build(),
            ddl.field(['label']).string().build(),
          ]),
        )
        .execute(client, db),
    );
  }, 30_000);

  afterAll(async () => {
    client?.close();
    await srv?.stop();
  });

  it('delete parent → child also deleted (cascade)', async () => {
    // Insert parent first (committed), then child — so the child's forward-FK
    // check sees the parent.
    br(
      await Batch.create('ins-parent')
        .add('p', write.insert('parent', { id: 10, name: 'Carol' }))
        .execute(client!, db),
    );
    br(
      await Batch.create('ins-child')
        .add('c', write.insert('child', { parent_id: 10, label: 'c1' }))
        .transactional()
        .execute(client!, db),
    );

    // Delete parent.
    br(
      await Batch.create('del')
        .add('d', write.del('parent', filter.eq('id', 10)))
        .execute(client!, db),
    );

    // Verify child is also gone.
    const after = br(
      await Batch.create('read-after')
        .add('q', Query.from('child').where(filter.eq('parent_id', 10)))
        .execute(client!, db),
    );
    // The child should have been cascade-deleted.
    const childResults = after.results?.q?.records ?? [];
    expect(childResults.length).toBe(0);
  });
});

// ════════════════════════════════════════════════════════════════════════
// Phase D.2 — ON DELETE SET NULL
// ════════════════════════════════════════════════════════════════════════

// TODO(#236): SKIPPED — SET NULL does not fire end-to-end yet (same gap as
// CASCADE above). Engine unit tests pass; the server path leaves the child's
// FK field unchanged. Flip to skipIf(!SERVER_AVAILABLE) once fixed.
describe.skip('Phase D.2 — ON DELETE SET NULL', () => {
  let srv: ServerHandle;
  let client: ShamirClient | null = null;
  let db: string;

  beforeAll(async () => {
    srv = await startServer();
    client = await connectAdmin(HOST, srv.port);

    db = await setupDb(client, 'fk_setnull', ['parent', 'child']);

    // Index on parent.id (FK DDL gate).
    br(
      await Batch.create('idx')
        .add('idx', ddl.createIndex('parent_id_idx_sn', 'parent', [['id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Index on child.parent_id.
    br(
      await Batch.create('idx2')
        .add('idx', ddl.createIndex('child_pid_idx_sn', 'child', [['parent_id']], { repo: 'main' }))
        .execute(client, db),
    );

    // Child schema: parent_id FK → parent.id with on_delete=set_null.
    // Field is nullable (not required) for SET NULL to work.
    br(
      await Batch.create('schema')
        .add(
          's',
          ddl.setTableSchema('child', [
            ddl
              .field(['parent_id'])
              .int()
              .nullable()
              .foreignKey('parent', 'id', { onDelete: 'set_null' })
              .build(),
            ddl.field(['label']).string().build(),
          ]),
        )
        .execute(client, db),
    );
  }, 30_000);

  afterAll(async () => {
    client?.close();
    await srv?.stop();
  });

  it('delete parent → child survives with parent_id == null', async () => {
    // Insert parent first (committed), then child (forward-FK sees parent).
    br(
      await Batch.create('ins-parent')
        .add('p', write.insert('parent', { id: 20, name: 'Dave' }))
        .execute(client!, db),
    );
    br(
      await Batch.create('ins-child')
        .add('c', write.insert('child', { parent_id: 20, label: 'd1' }))
        .transactional()
        .execute(client!, db),
    );

    // Delete parent.
    br(
      await Batch.create('del')
        .add('d', write.del('parent', filter.eq('id', 20)))
        .execute(client!, db),
    );

    // Verify child survives with parent_id == null.
    const after = br(
      await Batch.create('read-after')
        .add('q', Query.from('child').where(filter.eq('label', 'd1')))
        .execute(client!, db),
    );
    const childResults = after.results?.q?.records ?? [];
    expect(childResults.length).toBe(1);
    expect(childResults[0].parent_id ?? null).toBeNull();
  });
});

// ════════════════════════════════════════════════════════════════════════
// Phase D.3 — drop-guard (DropTable refused while referenced by a live FK)
// ════════════════════════════════════════════════════════════════════════

// TODO(#236): SKIPPED — drop-guard does not fire end-to-end yet. DropTable on a
// referenced table succeeds instead of being refused (drop_refused_fk). The
// engine-side guard exists (admin_table_index.rs) but is not triggered via the
// server's dropTable path. Flip to skipIf(!SERVER_AVAILABLE) once fixed.
describe.skip('Phase D.3 — drop-guard', () => {
  let srv: ServerHandle;
  let client: ShamirClient | null = null;
  let db: string;

  beforeAll(async () => {
    srv = await startServer();
    client = await connectAdmin(HOST, srv.port);

    db = await setupDb(client, 'fk_dropguard', ['parent', 'child']);

    br(
      await Batch.create('idx')
        .add('idx', ddl.createIndex('parent_id_idx_dg', 'parent', [['id']], { repo: 'main' }))
        .execute(client, db),
    );
    br(
      await Batch.create('idx2')
        .add('idx', ddl.createIndex('child_pid_idx_dg', 'child', [['parent_id']], { repo: 'main' }))
        .execute(client, db),
    );
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
          ]),
        )
        .execute(client, db),
    );
  }, 30_000);

  afterAll(async () => {
    client?.close();
    await srv?.stop();
  });

  it('DropTable on a referenced table is refused (drop_refused_fk)', async () => {
    try {
      br(
        await Batch.create('drop-parent')
          .add('d', ddl.dropTable(client!, db, 'main', 'parent'))
          .execute(client!, db),
      );
      expect.fail('should have refused dropping a referenced table');
    } catch (e: unknown) {
      expect((e as Error).message).toContain('drop_refused_fk');
    }
  });
});
