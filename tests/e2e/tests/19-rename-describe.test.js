/**
 * DDL singletons — rename_db + describe_table (Cluster E gaps).
 *
 * `rename_db`: catalogue re-key with guards (source must exist,
 * destination must NOT exist, SYSTEM_DB cannot be renamed).
 *
 * `describe_table`: full introspection in one response — asserts every
 * field of the response shape (admin_describe.rs ~line 197-210) reflects
 * what was actually configured.
 */

'use strict';

const { Query, write, ddl } = require('@shamir/client');

module.exports = async function ({ client, fixtures, test, assert, assertEq, assertThrows }) {
  // ── rename_db ────────────────────────────────────────────────────────

  test('rename_db: old name gone, new name serves same data', async () => {
    const oldName = await fixtures.setupDb(client, 'rn_data', ['items']);

    // Seed two rows so we can verify data survives the rename.
    await fixtures.seed(client, oldName, 'items', [
      { id: 'r1', name: 'alpha', qty: 10 },
      { id: 'r2', name: 'beta', qty: 20 },
    ]);

    const newName = oldName + '_renamed';

    const resp = await client.execute('default', {
      id: 'rn',
      queries: { r: ddl.renameDb(oldName, newName) },
    });
    const row = resp.results.r.records[0];
    assertEq(row.renamed_db, oldName);
    assertEq(row.to, newName);

    // Old name is gone — executing against it must fail.
    await assertThrows(() =>
      client.execute(oldName, {
        id: 'old-gone',
        queries: { q: Query.from('items').build() },
      }),
    );

    // New name serves the same data.
    const after = await client.execute(newName, {
      id: 'new-read',
      queries: { q: Query.from('items').build() },
    });
    const recs = after.results.q.records;
    assertEq(recs.length, 2);
    const ids = recs.map((r) => r.id).sort();
    assertEq(ids[0], 'r1');
    assertEq(ids[1], 'r2');
  });

  test('rename_db guard: nonexistent source errors', async () => {
    const ghost = fixtures.uniqueDbName('rn_ghost');
    await assertThrows(() =>
      client.execute('default', {
        id: 'rn-ghost',
        queries: { r: ddl.renameDb(ghost, ghost + '_x') },
      }),
    );
  });

  test('rename_db guard: existing destination errors', async () => {
    const dbA = await fixtures.setupDb(client, 'rn_dstA', []);
    const dbB = await fixtures.setupDb(client, 'rn_dstB', []);

    await assertThrows(() =>
      client.execute('default', {
        id: 'rn-collide',
        queries: { r: ddl.renameDb(dbA, dbB) },
      }),
    );
  });

  // ── describe_table ───────────────────────────────────────────────────

  test('describe_table: all 11 fields reflect configured state', async () => {
    const dbName = await fixtures.setupDb(client, 'desc_tbl', ['sensor']);
    const tbl = 'sensor';
    const repo = 'main';

    // 1. Set a declarative schema.
    const schemaRules = [
      { path: ['name'], type: 'string', required: true },
      { path: ['qty'], type: 'int', min: 0 },
    ];
    await client.execute(dbName, {
      id: 'set-schema',
      queries: { s: ddl.setTableSchema(tbl, schemaRules, { repo }) },
    });

    // 2. Create a secondary index.
    await client.execute(dbName, {
      id: 'mk-idx',
      queries: { i: ddl.createIndex('by_name', tbl, [['name']]) },
    });

    // 3. Set a buffer config with known values.
    const bufCfg = {
      max_bytes: 2097152,
      max_entries: 1000,
      ttl_ms: 5000,
      flush_interval_ms: 250,
      flush_batch_size: 64,
    };
    await client.execute(dbName, {
      id: 'set-buf',
      queries: { b: ddl.setBufferConfig(tbl, bufCfg, { repo }) },
    });

    // 4. describe_table — assert every field.
    const resp = await client.execute(dbName, {
      id: 'desc',
      queries: { d: ddl.describeTable(tbl, { repo }) },
    });
    const r = resp.results.d.records[0];

    // describe_table echo
    assertEq(r.describe_table, tbl);
    // repo echo
    assertEq(r.repo, repo);
    // schema — non-empty, contains our rule
    assert(Array.isArray(r.schema), `schema must be array, got ${typeof r.schema}`);
    assert(r.schema.length >= 2, `schema must have >= 2 rules, got ${r.schema.length}`);
    const nameRule = r.schema.find(
      (s) => Array.isArray(s.path) && s.path[0] === 'name',
    );
    assert(nameRule, 'schema must contain a rule for path ["name"]');
    assertEq(nameRule.type, 'string');
    assertEq(nameRule.required, true);
    // schema_version — positive integer after setting schema
    assert(typeof r.schema_version === 'number', 'schema_version must be a number');
    assert(r.schema_version > 0, `schema_version must be > 0, got ${r.schema_version}`);
    // indexes — non-empty, contains our index
    assert(Array.isArray(r.indexes), 'indexes must be array');
    const idxNames = r.indexes.map((i) => i.name);
    assert(
      idxNames.includes('by_name'),
      `index by_name missing: ${JSON.stringify(r.indexes)}`,
    );
    // validators — present (array, possibly empty)
    assert(Array.isArray(r.validators), 'validators must be array');
    // retention — present (null when never set)
    assert(r.retention === null || typeof r.retention === 'object',
      `retention unexpected: ${JSON.stringify(r.retention)}`);
    // buffer — non-null, matches what we set
    assert(r.buffer !== null && typeof r.buffer === 'object',
      'buffer must be an object');
    assertEq(r.buffer.max_bytes, 2097152);
    assertEq(r.buffer.max_entries, 1000);
    assertEq(r.buffer.ttl_ms, 5000);
    assertEq(r.buffer.flush_interval_ms, 250);
    assertEq(r.buffer.flush_batch_size, 64);
    // owner — present
    assert(r.owner !== undefined, 'owner must be present');
    assert(r.owner !== null, 'owner must not be null');
    // group — present (null or a value)
    assert(r.group === null || typeof r.group === 'number',
      `group unexpected: ${JSON.stringify(r.group)}`);
    // mode — present
    assert(typeof r.mode === 'number', `mode must be a number, got ${typeof r.mode}`);
  });
};
