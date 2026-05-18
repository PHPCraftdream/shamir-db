/**
 * Online table migration — full lifecycle e2e tests.
 *
 * Tests wire-format, HMAC gate, and the real migration lifecycle
 * (in-memory engine only for now).
 */

'use strict';

const hmac = require('../helpers/hmac');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {

  // ──────────────────────────────────────────────────────────────────
  // HMAC gate
  // ──────────────────────────────────────────────────────────────────

  test('start_migration without hmac → hmac_required', async () => {
    const db = await fixtures.setupDb(client, 'mig_no_hmac', ['users']);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            m: {
              start_migration: 'users',
              repo: 'main',
              dst_repo: 'cold',
              dst_engine: 'in_memory',
            },
          },
        }),
      (e) => /hmac_required/.test(e.message || ''),
      'expected hmac_required for start_migration without hmac'
    );
  });

  test('start_migration with wrong hmac → hmac_mismatch', async () => {
    const db = await fixtures.setupDb(client, 'mig_bad_hmac', ['users']);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            m: {
              start_migration: 'users',
              repo: 'main',
              dst_repo: 'cold',
              dst_engine: 'in_memory',
              hmac: 'aa'.repeat(32),
            },
          },
        }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch for start_migration with wrong hmac'
    );
  });

  test('start_migration hmac is bound to table name', async () => {
    const db = await fixtures.setupDb(client, 'mig_bind', ['a', 'b']);
    const op = hmac.start_migration_op(client, db, 'main', 'a', 'cold', 'in_memory');
    const tampered = { ...op, start_migration: 'b' };
    await assertThrows(
      () => client.execute(db, { id: 1, queries: { m: tampered } }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch when migration tag targets wrong table'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // Full lifecycle: start → status → commit → read from dst
  // ──────────────────────────────────────────────────────────────────

  test('full migration lifecycle (in_memory → in_memory)', async () => {
    const db = await fixtures.setupDb(client, 'mig_full', ['items']);

    // Seed data
    await client.execute(db, {
      id: 0,
      queries: {
        s1: { set: 'items', key: { id: 1 }, value: { id: 1, name: 'apple' } },
        s2: { set: 'items', key: { id: 2 }, value: { id: 2, name: 'banana' } },
        s3: { set: 'items', key: { id: 3 }, value: { id: 3, name: 'cherry' } },
      },
    });

    // Start migration
    const startOp = hmac.start_migration_op(client, db, 'main', 'items', 'archive', 'in_memory');
    const startResp = await client.execute(db, {
      id: 1,
      queries: { m: startOp },
    });
    const migId = startResp.results.m.records[0].migration_id;
    assert(typeof migId === 'string' && migId.length > 0, 'migration_id must be non-empty string');
    assertEq(startResp.results.m.records[0].phase, 'cutover_ready');

    // Check status
    const statusResp = await client.execute(db, {
      id: 2,
      queries: { s: { migration_status: migId } },
    });
    assertEq(statusResp.results.s.records[0].phase, 'cutover_ready');
    assertEq(statusResp.results.s.records[0].records_copied, 3);

    // Commit
    const commitOp = hmac.commit_migration_op(client, db, migId);
    const commitResp = await client.execute(db, {
      id: 3,
      queries: { c: commitOp },
    });
    assertEq(commitResp.results.c.records[0].phase, 'committed');
    assertEq(commitResp.results.c.records[0].src_records, 3);
    assertEq(commitResp.results.c.records[0].dst_records, 3);

    // Read from destination repo
    const readResp = await client.execute(db, {
      id: 4,
      queries: { r: { from: ['archive', 'items'] } },
    });
    assertEq(readResp.results.r.records.length, 3);
  });

  // ──────────────────────────────────────────────────────────────────
  // Rollback
  // ──────────────────────────────────────────────────────────────────

  test('migration rollback cleans up', async () => {
    const db = await fixtures.setupDb(client, 'mig_rb', ['items']);

    await client.execute(db, {
      id: 0,
      queries: { s: { set: 'items', key: { id: 1 }, value: { id: 1 } } },
    });

    const startOp = hmac.start_migration_op(client, db, 'main', 'items', 'rb_dst', 'in_memory');
    const startResp = await client.execute(db, {
      id: 1,
      queries: { m: startOp },
    });
    const migId = startResp.results.m.records[0].migration_id;

    // Rollback
    const rbOp = hmac.rollback_migration_op(client, db, migId);
    const rbResp = await client.execute(db, {
      id: 2,
      queries: { r: rbOp },
    });
    assertEq(rbResp.results.r.records[0].phase, 'rolled_back');

    // Status should fail — migration removed
    await assertThrows(
      () => client.execute(db, { id: 3, queries: { s: { migration_status: migId } } }),
      (e) => /not found/.test(e.message || ''),
      'status of rolled-back migration should fail'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // Unsupported engine → error
  // ──────────────────────────────────────────────────────────────────

  test('start_migration with unsupported engine → error', async () => {
    const db = await fixtures.setupDb(client, 'mig_bad_eng', ['items']);
    const op = hmac.start_migration_op(client, db, 'main', 'items', 'dst', 'postgres');
    await assertThrows(
      () => client.execute(db, { id: 1, queries: { m: op } }),
      (e) => /not.*supported/.test(e.message || ''),
      'expected unsupported engine error'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // migration_status (no hmac required — read-only)
  // ──────────────────────────────────────────────────────────────────

  test('migration_status of unknown ID → error', async () => {
    const db = await fixtures.setupDb(client, 'mig_404', []);
    await assertThrows(
      () => client.execute(db, { id: 1, queries: { s: { migration_status: 'nonexistent' } } }),
      (e) => /not found/.test(e.message || ''),
      'status of nonexistent migration should fail'
    );
  });
};
