/**
 * Admin DDL — create/drop db/repo/table/index, list.
 *
 * `list` op response shape (from `shamir_db/src/shamir_db/execute.rs`):
 *   list:databases → records: [{ databases: [name1, name2, ...] }]
 *   list:repos     → records: [{ repos: [...] }]
 *   list:tables    → records: [{ tables: [...], repo: '...' }]
 *   list:indexes   → records: [{ indexes: [...], repo, table }]
 *
 * One record per query, holding the listed names in an array under the
 * key matching the list type.
 *
 * `create_index` schema (admin/types.rs):
 *   { create_index: '<index_name>', table: '<table>', fields: [['col']],
 *     unique?: bool, repo?: '<repo>' }
 * — the value of `create_index` is the *name of the new index*, not
 * the table.
 */

'use strict';

const hmac = require('../helpers/hmac');
const { ddl } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {
  test('list databases includes default', async () => {
    const resp = await client.execute('default', {
      id: 'lsdb',
      queries: { l: ddl.listDatabases() },
    });
    const names = resp.results.l.records[0].databases;
    assert(Array.isArray(names), `expected an array, got ${JSON.stringify(names)}`);
    assert(names.includes('default'), `missing default in ${JSON.stringify(names)}`);
  });

  test('create_db then drop_db round-trip', async () => {
    const dbName = fixtures.uniqueDbName('ddl_drop');
    await client.execute('default', {
      id: 'mk',
      queries: { m: ddl.createDb(dbName) },
    });

    let resp = await client.execute('default', {
      id: 'lsdb2',
      queries: { l: ddl.listDatabases() },
    });
    assert(
      resp.results.l.records[0].databases.includes(dbName),
      `db ${dbName} not listed`
    );

    await client.execute('default', {
      id: 'rm',
      queries: { d: hmac.drop_db_op(client, dbName) },
    });

    resp = await client.execute('default', {
      id: 'lsdb3',
      queries: { l: ddl.listDatabases() },
    });
    assert(
      !resp.results.l.records[0].databases.includes(dbName),
      `db ${dbName} still listed after drop`
    );
  });

  test('create_repo + create_table + list', async () => {
    const dbName = await fixtures.setupDb(client, 'ddl_list', []);

    await client.execute(dbName, {
      id: 'r2',
      queries: { r: ddl.createRepo('cold') },
    });
    await client.execute(dbName, {
      id: 'tt',
      queries: {
        t1: ddl.createTable('users', { repo: 'main' }),
        t2: ddl.createTable('logs', { repo: 'cold' }),
      },
    });

    const reposResp = await client.execute(dbName, {
      id: 'lsr',
      queries: { l: ddl.listRepos() },
    });
    const repoNames = reposResp.results.l.records[0].repos.sort();
    assertEq(repoNames.length, 2);
    assert(repoNames.includes('main'));
    assert(repoNames.includes('cold'));

    // list:tables is repo-scoped — separate query per repo.
    const mainTables = await client.execute(dbName, {
      id: 'lst-main',
      queries: { l: ddl.listTables({ repo: 'main' }) },
    });
    assert(
      mainTables.results.l.records[0].tables.includes('users'),
      `users missing in main: ${JSON.stringify(mainTables.results.l.records[0])}`
    );

    const coldTables = await client.execute(dbName, {
      id: 'lst-cold',
      queries: { l: ddl.listTables({ repo: 'cold' }) },
    });
    assert(
      coldTables.results.l.records[0].tables.includes('logs'),
      `logs missing in cold: ${JSON.stringify(coldTables.results.l.records[0])}`
    );
  });

  test('create_index + list + drop_index', async () => {
    const dbName = await fixtures.setupDb(client, 'ddl_idx', ['t']);

    await client.execute(dbName, {
      id: 'mk-idx',
      queries: {
        i: ddl.createIndex('by_email', 't', [['email']]),
      },
    });

    const lsResp = await client.execute(dbName, {
      id: 'ls-idx',
      queries: { l: ddl.listIndexes('t', { repo: 'main' }) },
    });
    // list:indexes returns rich entries (name + unique flag), unlike
    // databases/repos/tables which return plain string arrays.
    const indexNames = lsResp.results.l.records[0].indexes.map((i) => i.name);
    assert(
      indexNames.includes('by_email'),
      `index by_email not listed: ${JSON.stringify(lsResp.results.l.records[0].indexes)}`
    );

    await client.execute(dbName, {
      id: 'rm-idx',
      queries: { d: hmac.drop_index_op(client, dbName, 'main', 't', 'by_email') },
    });

    const ls2 = await client.execute(dbName, {
      id: 'ls-idx2',
      queries: { l: ddl.listIndexes('t', { repo: 'main' }) },
    });
    const afterNames = ls2.results.l.records[0].indexes.map((i) => i.name);
    assert(
      !afterNames.includes('by_email'),
      `index still listed after drop: ${JSON.stringify(ls2.results.l.records[0].indexes)}`
    );
  });

  test('drop_index if_exists: existing drop → re-drop → if_exists no-op', async () => {
    const dbName = await fixtures.setupDb(client, 'ddl_drop_ie', ['t']);

    await client.execute(dbName, {
      id: 'mk-idx',
      queries: { i: ddl.createIndex('to_drop', 't', [['x']]) },
    });

    // Step 1: drop the EXISTING index — must report existed:true.
    const drop1 = await client.execute(dbName, {
      id: 'drop1',
      queries: { d: hmac.drop_index_op(client, dbName, 'main', 't', 'to_drop') },
    });
    assertEq(drop1.results.d.records[0].existed, true);

    // Step 2: re-drop WITHOUT if_exists. The table still exists, so the
    // server resolves the table but finds no matching index → silently
    // returns existed:false (does NOT error — see admin_table_index.rs
    // ~line 508-520: drop_index returns Ok(false), not an error).
    const drop2 = await client.execute(dbName, {
      id: 'drop2',
      queries: { d: hmac.drop_index_op(client, dbName, 'main', 't', 'to_drop') },
    });
    assertEq(drop2.results.d.records[0].existed, false);

    // Step 3: re-drop WITH if_exists:true — clean no-op, existed:false.
    const drop3 = await client.execute(dbName, {
      id: 'drop3',
      queries: {
        d: hmac.drop_index_op(client, dbName, 'main', 't', 'to_drop', {
          if_exists: true,
        }),
      },
    });
    assertEq(drop3.results.d.records[0].existed, false);
  });

  test('drop_index if_exists: non-existent table without if_exists errors, with if_exists no-op', async () => {
    const dbName = await fixtures.setupDb(client, 'ddl_drop_ie_tbl', []);

    // Drop index from a NON-EXISTENT table WITHOUT if_exists → the
    // server fails at table resolution and returns an error.
    await assertThrows(() =>
      client.execute(dbName, {
        id: 'drop-missing-strict',
        queries: {
          d: hmac.drop_index_op(client, dbName, 'main', 'no_such_table', 'idx'),
        },
      }),
    );

    // Same drop WITH if_exists:true → the early-exit guard short-circuits
    // before table resolution, returning a clean existed:false no-op
    // (admin_table_index.rs ~line 447-471).
    const resp = await client.execute(dbName, {
      id: 'drop-missing-if-exists',
      queries: {
        d: hmac.drop_index_op(
          client,
          dbName,
          'main',
          'no_such_table',
          'idx',
          { if_exists: true },
        ),
      },
    });
    assertEq(resp.results.d.records[0].existed, false);
  });
};
