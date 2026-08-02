/**
 * Per-test fixture helpers — keeps test files terse.
 *
 * Each test file gets its own database (so no cross-contamination)
 * and can ask for a freshly-created repo+table inside it. Builders
 * return JSON-serializable BatchRequest objects ready to pass to
 * `client.execute(...)`.
 *
 * Wire ops are constructed via the platform-agnostic query builders in
 * `@shamir/client` (the TS package) rather than hand-rolled object
 * literals, per the repo's "Query construction — builder only" rule.
 * `tests/e2e` is CommonJS while `@shamir/client` ships ESM, but Node
 * 22.12+ can `require()` an ESM module that has no top-level await —
 * so the builders are loaded with a plain synchronous `require()`,
 * which keeps these helpers (and their call sites) synchronous.
 */

'use strict';

const { ddl, write } = require('@shamir/client');

let counter = 0;
function uniqueDbName(label) {
  counter += 1;
  return `t_${label}_${process.pid}_${counter}`;
}

/**
 * Create a fresh database (in `default`) and inside it a `main` repo
 * with the requested tables.
 */
async function setupDb(client, label, tableNames = ['items']) {
  const db = uniqueDbName(label);

  // Step 1: create the database (must run against `default` since the
  // target db doesn't exist yet).
  await client.execute('default', {
    id: `setup-${db}-db`,
    queries: { mk: ddl.createDb(db) },
  });

  // Step 2: create the repo + tables inside it.
  const queries = { mr: ddl.createRepo('main') };
  for (let i = 0; i < tableNames.length; i += 1) {
    queries[`tb${i}`] = ddl.createTable(tableNames[i], { repo: 'main' });
  }
  await client.execute(db, {
    id: `setup-${db}-tables`,
    queries,
  });

  return db;
}

/**
 * Bulk-seed records into a table via a single batch of `set` ops.
 * `records` must be an array of objects each carrying the values that
 * uniquely identify the row under the supplied `keyFields`.
 */
async function seed(client, db, table, records, keyFields = ['id']) {
  const queries = {};
  records.forEach((r, i) => {
    const key = {};
    for (const k of keyFields) key[k] = r[k];
    queries[`s${i}`] = write.upsert(table, key, r);
  });
  return client.execute(db, { id: `seed-${db}-${table}`, queries });
}

module.exports = { setupDb, seed, uniqueDbName };
