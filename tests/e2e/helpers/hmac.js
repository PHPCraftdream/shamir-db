/**
 * HMAC-confirmation helpers for destructive DDL/admin ops.
 *
 * The server gates every drop_* / migration op behind an HMAC tag whose
 * canonical input is null-byte-separated identifier bytes; the key is
 * derived from `session_id` via a domain-separated SHA-256. Both sides
 * MUST agree byte-for-byte — these helpers mirror
 * `crates/shamir-query-types/src/hmac.rs`.
 *
 * The op construction itself is delegated to the platform-agnostic
 * query builders in `@shamir/client` (`ddl.*` / `admin.*`), which carry
 * their OWN copy of the canonical-byte layout (mirroring the same Rust
 * `hmac.rs`). Routing through them removes the duplication risk of two
 * hand-rolled canonical encoders drifting out of sync. A `signerFor`
 * adapter wraps this file's `sign()` so the builders call back into the
 * EXACT same key-derivation + HMAC-SHA256 pipeline — the tag bytes a
 * builder produces are identical to what the old inline construction did
 * (verified by an equivalence probe).
 *
 * `tests/e2e` is CommonJS while `@shamir/client` ships ESM, but Node
 * 22.12+ can `require()` an ESM module that has no top-level await, so
 * the builders load synchronously and every `*_op` helper stays a plain
 * (non-async) function returning the wire object directly — call sites
 * like `queries: { d: drop_table_op(...) }` are unchanged.
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
 * pre-computed `hmac` field, ready to drop into a batch. The public
 * names and call signatures are stable.
 */

'use strict';

const crypto = require('crypto');
const { ddl, admin } = require('@shamir/client');

// ── Signing primitives (single source of truth for the HMAC tag) ────

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

/**
 * Adapter that satisfies the `HmacSigner` interface the @shamir/client
 * builders expect (`{ hmacTagHex(canonical: Uint8Array): string }`) by
 * routing the canonical bytes through this file's `sign()`. The signing
 * logic is NOT reimplemented — the builder owns canonical construction,
 * this helper owns key derivation + the HMAC primitive.
 */
function signerFor(client) {
  return { hmacTagHex: (canonical) => sign(client, Buffer.from(canonical)) };
}

// ── HMAC-gated ops (delegated to @shamir/client builders) ───────────

/**
 * Build a `drop_db` op with HMAC attached. Delegates to
 * `ddl.dropDb`; `opts.cascade` toggles recursive removal (the `cascade`
 * flag is NOT part of the signed canonical bytes, so the same tag is
 * valid for both forms — see `admin_db_repo.rs::handle_drop_db`).
 */
function drop_db_op(client, dbName, opts = {}) {
  return ddl.dropDb(signerFor(client), dbName, opts);
}

/** Build a `drop_repo` op with HMAC attached. `dbInUse` is the db the
 *  batch will be `execute()`d against. */
function drop_repo_op(client, dbInUse, repo) {
  return ddl.dropRepo(signerFor(client), dbInUse, repo);
}

/** Build a `drop_table` op with HMAC attached. */
function drop_table_op(client, dbInUse, repo, table) {
  return ddl.dropTable(signerFor(client), dbInUse, repo, table);
}

/** Build a `drop_index` op with HMAC attached. */
function drop_index_op(client, dbInUse, repo, table, indexName, opts = {}) {
  return ddl.dropIndex(signerFor(client), dbInUse, repo, table, indexName, opts);
}

/** Build a `drop_user` op with HMAC attached. */
function drop_user_op(client, username) {
  return admin.dropUser(signerFor(client), username);
}

/**
 * Build a `drop_role` op with HMAC attached.
 *
 * NOTE: there is NO matching builder in @shamir/client — roles became
 * plain string labels attached to directory users (task #549), so the
 * only role-mutating builders are `grantRole` / `revokeRole`; a
 * `drop_role` wire op no longer exists on the server side
 * (`shamir-query-types` has no `canonical_drop_role`). This helper is
 * retained for signature stability but is legacy/unused, so its
 * canonical bytes are still constructed inline here.
 */
function drop_role_op(client, role) {
  const canonical = joinNullBytes(['drop_role', role]);
  return { drop_role: role, hmac: sign(client, canonical) };
}

/** Build a `start_migration` op with HMAC attached. */
function start_migration_op(client, dbInUse, srcRepo, table, dstRepo, dstEngine, opts = {}) {
  return ddl.startMigration(signerFor(client), dbInUse, srcRepo, table, dstRepo, dstEngine, opts);
}

/** Build a `commit_migration` op with HMAC attached. */
function commit_migration_op(client, dbInUse, migrationId) {
  return ddl.commitMigration(signerFor(client), dbInUse, migrationId);
}

/** Build a `rollback_migration` op with HMAC attached. */
function rollback_migration_op(client, dbInUse, migrationId) {
  return ddl.rollbackMigration(signerFor(client), dbInUse, migrationId);
}

module.exports = {
  deriveKey,
  joinNullBytes,
  sign,
  signerFor,
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
