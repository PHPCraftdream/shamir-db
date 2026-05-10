/**
 * Per-test fixture helpers — keeps test files terse.
 *
 * Each test file gets its own database (so no cross-contamination)
 * and can ask for a freshly-created repo+table inside it. Builders
 * return JSON-serializable BatchRequest objects ready to pass to
 * `client.execute(...)`.
 */

'use strict';

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
    queries: { mk: { create_db: db } },
  });

  // Step 2: create the repo + tables inside it.
  const queries = { mr: { create_repo: 'main' } };
  for (let i = 0; i < tableNames.length; i += 1) {
    queries[`tb${i}`] = { create_table: tableNames[i], repo: 'main' };
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
    queries[`s${i}`] = { set: table, key, value: r };
  });
  return client.execute(db, { id: `seed-${db}-${table}`, queries });
}

module.exports = { setupDb, seed, uniqueDbName };
