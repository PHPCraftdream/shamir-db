'use strict';
const crypto = require('crypto');
const { ddl, admin } = require('@shamir/client');

// The client builders own the v2 canonical encoder; this helper signs only
// those bytes with the per-session key used by Rust and TypeScript.
function deriveKey(sessionId) {
  return crypto.createHash('sha256').update('shamir-db hmac key v1\0', 'utf8').update(sessionId).digest();
}
function sign(client, canonical) {
  return crypto.createHmac('sha256', deriveKey(client.sessionId())).update(canonical).digest('hex');
}
function signerFor(client) { return { hmacTagHex: canonical => sign(client, Buffer.from(canonical)) }; }
function drop_db_op(client, db, opts = {}) {
  return ddl.dropDb(signerFor(client), db, opts);
}
function drop_repo_op(client, db, repo, opts = {}) {
  return ddl.dropRepo(signerFor(client), db, repo, opts);
}
function drop_table_op(client, db, repo, table, opts = {}) {
  return ddl.dropTable(signerFor(client), db, repo, table, opts);
}
function drop_index_op(client, db, repo, table, index, opts = {}) {
  return ddl.dropIndex(signerFor(client), db, repo, table, index, opts);
}
function drop_user_op(client, user) { return admin.dropUser(signerFor(client), user); }
function start_migration_op(client, db, repo, table, dstRepo, dstEngine, opts = {}) {
  return ddl.startMigration(signerFor(client), db, repo, table, dstRepo, dstEngine, opts);
}
function commit_migration_op(client, db, id) {
  return ddl.commitMigration(signerFor(client), db, id);
}
function rollback_migration_op(client, db, id) {
  return ddl.rollbackMigration(signerFor(client), db, id);
}
module.exports = { deriveKey, sign, signerFor, drop_db_op, drop_repo_op, drop_table_op, drop_index_op, drop_user_op, start_migration_op, commit_migration_op, rollback_migration_op };
