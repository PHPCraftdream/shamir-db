/**
 * HMAC gate — destructive ops require a session-derived tag.
 *
 * Wire-side error codes from `db_handler.rs`:
 *   - `hmac_required` — destructive op missing the `hmac` field
 *   - `hmac_mismatch` — present but doesn't match canonical input
 *
 * Canonical inputs and the key derivation are defined in
 * `crates/shamir-query-types/src/hmac.rs` and mirrored by
 * `tests/e2e/helpers/hmac.js` (used here for the "happy path").
 */

'use strict';

const hmac = require('../helpers/hmac');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {
  // The napi binding throws on `DbResponse::Error` — assertThrows
  // catches it and we sniff the `.code`-bearing message for the
  // wire code from db_handler.rs.

  test('drop_table without hmac → hmac_required', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_miss_table', ['t']);
    await assertThrows(
      () =>
        client.execute(dbName, {
          id: 1,
          queries: { d: { drop_table: 't', repo: 'main' } },
        }),
      (e) => /hmac_required/.test(e.message || ''),
      'expected hmac_required error'
    );
  });

  test('drop_table with wrong hmac → hmac_mismatch', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_wrong_table', ['t']);
    await assertThrows(
      () =>
        client.execute(dbName, {
          id: 1,
          queries: {
            d: {
              drop_table: 't',
              repo: 'main',
              hmac: 'aa'.repeat(32), // 64 hex chars, definitely wrong
            },
          },
        }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch error'
    );
  });

  test('drop_table with correct hmac succeeds', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_ok_table', ['t']);
    const resp = await client.execute(dbName, {
      id: 1,
      queries: { d: hmac.drop_table_op(client, dbName, 'main', 't') },
    });
    const row = resp.results.d.records[0];
    assertEq(row.dropped_table, 't');
    assertEq(row.existed, true);
  });

  test('drop_db without hmac → hmac_required', async () => {
    const victim = await fixtures.setupDb(client, 'hmac_miss_db', []);
    await assertThrows(
      () =>
        client.execute('default', {
          id: 1,
          queries: { d: { drop_db: victim } },
        }),
      (e) => /hmac_required/.test(e.message || ''),
      'expected hmac_required error'
    );
  });

  test('drop_db with correct hmac succeeds', async () => {
    // `setupDb` always creates a `main` repo, so the db is non-empty and
    // `drop_db` now requires `cascade: true` (referential-integrity guard
    // added in the replication campaign — see
    // `admin_db_repo.rs::handle_drop_db` /
    // `ddl_wire_e2e/error_codes.rs::error_code_still_referenced_drop_db`).
    // The HMAC canonical input is `b"drop_db\0<db>"`; `cascade` is not part
    // of the signed bytes, so the tag is the same.
    const victim = await fixtures.setupDb(client, 'hmac_ok_db', []);
    const resp = await client.execute('default', {
      id: 1,
      queries: { d: hmac.drop_db_op(client, victim, { cascade: true }) },
    });
    const row = resp.results.d.records[0];
    assertEq(row.dropped, victim);
  });

  test('drop_db without cascade on a db with repos → still_referenced', async () => {
    // Pin the new referential-integrity contract: a db that still owns a
    // repo cannot be dropped without `cascade`. The HMAC is valid (correct
    // tag), so this proves the `still_referenced` guard fires AFTER the
    // HMAC gate — i.e. it is a business rule, not an auth failure.
    const victim = await fixtures.setupDb(client, 'hmac_ref', []);
    let err = null;
    try {
      await client.execute('default', {
        id: 1,
        queries: { d: hmac.drop_db_op(client, victim) },
      });
    } catch (e) {
      err = e;
    }
    assert(err, 'expected drop_db without cascade to fail with still_referenced');
    assert(
      /still_referenced/.test(err.message || ''),
      `expected still_referenced, got: ${err.message}`
    );
  });

  test('drop_index with correct hmac succeeds', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_ok_idx', ['t']);
    await client.execute(dbName, {
      id: 0,
      queries: {
        i: { create_index: 'by_x', table: 't', fields: [['x']] },
      },
    });
    const resp = await client.execute(dbName, {
      id: 1,
      queries: {
        d: hmac.drop_index_op(client, dbName, 'main', 't', 'by_x'),
      },
    });
    assertEq(resp.results.d.records[0].dropped_index, 'by_x');
    assertEq(resp.results.d.records[0].existed, true);
  });

  test('drop_index unique=true requires its own tag flavour', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_idx_uniq', ['t']);
    await client.execute(dbName, {
      id: 0,
      queries: {
        i: { create_index: 'by_em', table: 't', fields: [['email']], unique: true },
      },
    });

    // Compute a tag for the non-unique form but submit unique=true —
    // server must reject as hmac_mismatch.
    const wrong = hmac.drop_index_op(client, dbName, 'main', 't', 'by_em'); // unique:false default
    wrong.unique = true; // tamper with the op without re-signing
    await assertThrows(
      () => client.execute(dbName, { id: 1, queries: { d: wrong } }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch error after tampering unique flag'
    );

    // Correct: regenerate with unique:true.
    const correct = hmac.drop_index_op(client, dbName, 'main', 't', 'by_em', {
      unique: true,
    });
    const ok = await client.execute(dbName, { id: 2, queries: { d: correct } });
    assertEq(ok.results.d.records[0].dropped_index, 'by_em');
  });

  test('mixed batch: one drop without hmac fails the whole batch', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_mixed', ['t']);
    await assertThrows(
      () =>
        client.execute(dbName, {
          id: 1,
          queries: {
            r: { from: 't' },
            d: { drop_table: 't', repo: 'main' },
          },
        }),
      (e) => /hmac_required/.test(e.message || ''),
      'expected the unsigned drop to fail the whole batch'
    );
  });

  test('read op needs no hmac', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_read', ['t']);
    const resp = await client.execute(dbName, {
      id: 1,
      queries: { r: { from: 't' } },
    });
    assertEq(resp.results.r.records.length, 0);
  });

  test('create_table needs no hmac', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_ctable', []);
    const resp = await client.execute(dbName, {
      id: 1,
      queries: { t: { create_table: 'q', repo: 'main' } },
    });
    assertEq(resp.results.t.records[0].created_table, 'q');
  });

  test('tag bound to target — drop_table for table A does not work on table B', async () => {
    const dbName = await fixtures.setupDb(client, 'hmac_bind', ['a', 'b']);
    // Sign a tag for table "a".
    const opForA = hmac.drop_table_op(client, dbName, 'main', 'a');
    // Send it but target table "b" — keep the same hmac field.
    const tampered = {
      drop_table: 'b',
      repo: 'main',
      hmac: opForA.hmac,
    };
    await assertThrows(
      () => client.execute(dbName, { id: 1, queries: { d: tampered } }),
      (e) => /hmac_mismatch/.test(e.message || ''),
      'expected hmac_mismatch when tag is for a different target'
    );
  });
};
