/**
 * HMAC-confirmation helpers for destructive DDL ops.
 *
 * The server gates every drop_* op behind an HMAC tag whose
 * canonical input is null-byte-separated identifier bytes; the
 * key is derived from `session_id` via a domain-separated
 * SHA-256. Both sides MUST agree byte-for-byte — these helpers
 * mirror `crates/shamir-query-types/src/hmac.rs`.
 *
 * Usage:
 *
 *   const { drop_table_op } = require('../helpers/hmac');
 *   await client.execute('mydb', {
 *     id: 1,
 *     queries: { d: drop_table_op(client, 'mydb', 'main', 'items') },
 *   });
 *
 * Each `*_op` helper returns the FULL op object including the
 * pre-computed `hmac` field, ready to drop into a batch.
 */

'use strict';

const crypto = require('crypto');

function deriveKey(sessionId) {
  // sessionId arrives from the napi binding as a Buffer of length 32.
  const h = crypto.createHash('sha256');
  h.update('shamir-db hmac key v1\0', 'utf8');
  h.update(sessionId);
  return h.digest();
}

function joinNullBytes(parts) {
  // Each `parts` entry is either a string or a Buffer; we encode
  // strings as UTF-8 and join with single 0x00 bytes between.
  const buffs = parts.map((p) =>
    Buffer.isBuffer(p) ? p : Buffer.from(String(p), 'utf8')
  );
  const out = [];
  for (let i = 0; i < buffs.length; i += 1) {
    if (i > 0) out.push(Buffer.from([0]));
    out.push(buffs[i]);
  }
  return Buffer.concat(out);
}

function sign(client, canonical) {
  const key = deriveKey(client.sessionId());
  return crypto.createHmac('sha256', key).update(canonical).digest('hex');
}

/** Build a `drop_db` op with HMAC attached. */
function drop_db_op(client, dbName) {
  const canonical = joinNullBytes(['drop_db', dbName]);
  return { drop_db: dbName, hmac: sign(client, canonical) };
}

/** Build a `drop_repo` op with HMAC attached.
 *  `dbInUse` is the db the batch will be `execute()`d against. */
function drop_repo_op(client, dbInUse, repo) {
  const canonical = joinNullBytes(['drop_repo', dbInUse, repo]);
  return { drop_repo: repo, hmac: sign(client, canonical) };
}

/** Build a `drop_table` op with HMAC attached. */
function drop_table_op(client, dbInUse, repo, table) {
  const canonical = joinNullBytes(['drop_table', dbInUse, repo, table]);
  return {
    drop_table: table,
    repo,
    hmac: sign(client, canonical),
  };
}

/** Build a `drop_index` op with HMAC attached. */
function drop_index_op(client, dbInUse, repo, table, indexName, opts = {}) {
  const unique = !!opts.unique;
  const canonical = joinNullBytes([
    'drop_index',
    dbInUse,
    repo,
    table,
    indexName,
    unique ? '1' : '0',
  ]);
  const op = {
    drop_index: indexName,
    table,
    repo,
    hmac: sign(client, canonical),
  };
  if (unique) op.unique = true;
  return op;
}

/** Build a `drop_user` op with HMAC attached. */
function drop_user_op(client, username) {
  const canonical = joinNullBytes(['drop_user', username]);
  return { drop_user: username, hmac: sign(client, canonical) };
}

/** Build a `drop_role` op with HMAC attached. */
function drop_role_op(client, role) {
  const canonical = joinNullBytes(['drop_role', role]);
  return { drop_role: role, hmac: sign(client, canonical) };
}

/** Build a `start_migration` op with HMAC attached. */
function start_migration_op(client, dbInUse, srcRepo, table, dstRepo, dstEngine, opts = {}) {
  const canonical = joinNullBytes([
    'start_migration',
    dbInUse,
    srcRepo,
    table,
    dstRepo,
    dstEngine,
  ]);
  const op = {
    start_migration: table,
    repo: srcRepo,
    dst_repo: dstRepo,
    dst_engine: dstEngine,
    hmac: sign(client, canonical),
  };
  if (opts.dst_path) op.dst_path = opts.dst_path;
  return op;
}

/** Build a `commit_migration` op with HMAC attached. */
function commit_migration_op(client, dbInUse, migrationId) {
  const canonical = joinNullBytes(['commit_migration', dbInUse, migrationId]);
  return { commit_migration: migrationId, hmac: sign(client, canonical) };
}

/** Build a `rollback_migration` op with HMAC attached. */
function rollback_migration_op(client, dbInUse, migrationId) {
  const canonical = joinNullBytes(['rollback_migration', dbInUse, migrationId]);
  return { rollback_migration: migrationId, hmac: sign(client, canonical) };
}

module.exports = {
  deriveKey,
  joinNullBytes,
  sign,
  drop_db_op,
  drop_repo_op,
  drop_table_op,
  drop_index_op,
  drop_user_op,
  drop_role_op,
  start_migration_op,
  commit_migration_op,
  rollback_migration_op,
};
