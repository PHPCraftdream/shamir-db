/**
 * Online table migration — Phase A stubs.
 *
 * Tests that the wire-format is accepted, HMAC gate works, and the
 * server returns "not yet implemented" until Phase B lands.
 */

'use strict';

const hmac = require('../helpers/hmac');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {

  // ──────────────────────────────────────────────────────────────────
  // start_migration
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
              dst_engine: 'redb',
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
              dst_engine: 'redb',
              hmac: 'aa'.repeat(32),
            },
          },
        }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch for start_migration with wrong hmac'
    );
  });

  test('start_migration with correct hmac → not yet implemented', async () => {
    const db = await fixtures.setupDb(client, 'mig_stub', ['users']);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            m: hmac.start_migration_op(client, db, 'main', 'users', 'cold', 'redb'),
          },
        }),
      (e) => /not yet implemented/.test(e.message || ''),
      'expected not-yet-implemented for start_migration Phase A stub'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // commit_migration
  // ──────────────────────────────────────────────────────────────────

  test('commit_migration with correct hmac → not yet implemented', async () => {
    const db = await fixtures.setupDb(client, 'mig_commit', []);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            c: hmac.commit_migration_op(client, db, 'mig-001'),
          },
        }),
      (e) => /not yet implemented/.test(e.message || ''),
      'expected not-yet-implemented for commit_migration Phase A stub'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // rollback_migration
  // ──────────────────────────────────────────────────────────────────

  test('rollback_migration with correct hmac → not yet implemented', async () => {
    const db = await fixtures.setupDb(client, 'mig_rollback', []);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            r: hmac.rollback_migration_op(client, db, 'mig-001'),
          },
        }),
      (e) => /not yet implemented/.test(e.message || ''),
      'expected not-yet-implemented for rollback_migration Phase A stub'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // migration_status (no hmac required — read-only)
  // ──────────────────────────────────────────────────────────────────

  test('migration_status → not yet implemented', async () => {
    const db = await fixtures.setupDb(client, 'mig_status', []);
    await assertThrows(
      () =>
        client.execute(db, {
          id: 1,
          queries: {
            s: { migration_status: 'mig-001' },
          },
        }),
      (e) => /not yet implemented/.test(e.message || ''),
      'expected not-yet-implemented for migration_status Phase A stub'
    );
  });

  // ──────────────────────────────────────────────────────────────────
  // HMAC binding — tag for table A can't start migration for table B
  // ──────────────────────────────────────────────────────────────────

  test('start_migration hmac is bound to table name', async () => {
    const db = await fixtures.setupDb(client, 'mig_bind', ['a', 'b']);
    const op = hmac.start_migration_op(client, db, 'main', 'a', 'cold', 'redb');
    // Tamper: send the tag signed for table "a" but target table "b".
    const tampered = { ...op, start_migration: 'b' };
    await assertThrows(
      () => client.execute(db, { id: 1, queries: { m: tampered } }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch when migration tag targets wrong table'
    );
  });
};
