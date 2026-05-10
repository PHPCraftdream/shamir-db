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

module.exports = async function ({ client, fixtures, test, assert, assertEq }) {
  test('list databases includes default', async () => {
    const resp = await client.execute('default', {
      id: 'lsdb',
      queries: { l: { list: 'databases' } },
    });
    const names = resp.results.l.records[0].databases;
    assert(Array.isArray(names), `expected an array, got ${JSON.stringify(names)}`);
    assert(names.includes('default'), `missing default in ${JSON.stringify(names)}`);
  });

  test('create_db then drop_db round-trip', async () => {
    const dbName = fixtures.uniqueDbName('ddl_drop');
    await client.execute('default', {
      id: 'mk',
      queries: { m: { create_db: dbName } },
    });

    let resp = await client.execute('default', {
      id: 'lsdb2',
      queries: { l: { list: 'databases' } },
    });
    assert(
      resp.results.l.records[0].databases.includes(dbName),
      `db ${dbName} not listed`
    );

    await client.execute('default', {
      id: 'rm',
      queries: { d: { drop_db: dbName } },
    });

    resp = await client.execute('default', {
      id: 'lsdb3',
      queries: { l: { list: 'databases' } },
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
      queries: { r: { create_repo: 'cold' } },
    });
    await client.execute(dbName, {
      id: 'tt',
      queries: {
        t1: { create_table: 'users', repo: 'main' },
        t2: { create_table: 'logs', repo: 'cold' },
      },
    });

    const reposResp = await client.execute(dbName, {
      id: 'lsr',
      queries: { l: { list: 'repos' } },
    });
    const repoNames = reposResp.results.l.records[0].repos.sort();
    assertEq(repoNames.length, 2);
    assert(repoNames.includes('main'));
    assert(repoNames.includes('cold'));

    // list:tables is repo-scoped — separate query per repo.
    const mainTables = await client.execute(dbName, {
      id: 'lst-main',
      queries: { l: { list: 'tables', repo: 'main' } },
    });
    assert(
      mainTables.results.l.records[0].tables.includes('users'),
      `users missing in main: ${JSON.stringify(mainTables.results.l.records[0])}`
    );

    const coldTables = await client.execute(dbName, {
      id: 'lst-cold',
      queries: { l: { list: 'tables', repo: 'cold' } },
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
        i: {
          create_index: 'by_email', // index name
          table: 't',
          fields: [['email']],
        },
      },
    });

    const lsResp = await client.execute(dbName, {
      id: 'ls-idx',
      queries: { l: { list: 'indexes', repo: 'main', table: 't' } },
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
      queries: { d: { drop_index: 'by_email', table: 't' } },
    });

    const ls2 = await client.execute(dbName, {
      id: 'ls-idx2',
      queries: { l: { list: 'indexes', repo: 'main', table: 't' } },
    });
    const afterNames = ls2.results.l.records[0].indexes.map((i) => i.name);
    assert(
      !afterNames.includes('by_email'),
      `index still listed after drop: ${JSON.stringify(ls2.results.l.records[0].indexes)}`
    );
  });
};
