/**
 * Canonical HMAC input bytes + tag computation for destructive admin ops.
 *
 * Byte-for-byte mirror of `crates/shamir-query-types/src/hmac.rs` and the
 * e2e helper `tests/e2e/helpers/hmac.js`. Server and client MUST agree
 * exactly — changing a layout here is a breaking protocol change.
 *
 * The HMAC on `drop_*` / migration ops is a "did you mean it" intent guard,
 * not an auth gate (the session_id is already the bearer token). The client
 * cannot produce a matching tag by accident.
 *
 * Key derivation: `key = SHA256("shamir-db hmac key v1\0" || session_id)`.
 * Per-op canonical input: null-byte-separated identifier bytes.
 *
 * PLATFORM-AGNOSTIC: crypto is delegated to the injected `Platform`
 * (sha256 / hmacSha256). `TextEncoder` is a Web standard available in both
 * Node and browsers (same footing as the `@msgpack/msgpack` import in core).
 */

import type { Platform } from './platform.js';

const utf8 = new TextEncoder();

/** Domain-separation prefix for the session HMAC key (includes trailing NUL). */
const KEY_DOMAIN = utf8.encode('shamir-db hmac key v1\0');

/** Concatenate byte chunks into one Uint8Array. */
function concatBytes(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  return out;
}

/**
 * Join parts with single 0x00 bytes between them (no leading/trailing NUL).
 * String parts are UTF-8 encoded; byte parts are used as-is.
 */
export function joinNull(parts: Array<string | Uint8Array>): Uint8Array {
  const encoded = parts.map((p) => (typeof p === 'string' ? utf8.encode(p) : p));
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < encoded.length; i += 1) {
    if (i > 0) chunks.push(new Uint8Array([0]));
    chunks.push(encoded[i]!);
  }
  return concatBytes(chunks);
}

/** Lowercase hex encoding (matches the Rust `hex_encode`). */
function hexEncode(bytes: Uint8Array): string {
  const table = '0123456789abcdef';
  let s = '';
  for (const b of bytes) {
    s += table[(b >> 4) & 0x0f]! + table[b & 0x0f]!;
  }
  return s;
}

/**
 * Derive the 32-byte session HMAC key from the session bearer token:
 * `SHA256("shamir-db hmac key v1\0" || session_id)`.
 */
export function deriveSessionHmacKey(
  platform: Platform,
  sessionId: Uint8Array,
): Uint8Array {
  return platform.sha256(concatBytes([KEY_DOMAIN, sessionId]));
}

/** Hex-encoded HMAC-SHA256 tag over `canonical` keyed by `key`. */
export function computeTagHex(
  platform: Platform,
  key: Uint8Array,
  canonical: Uint8Array,
): string {
  return hexEncode(platform.hmacSha256(key, canonical));
}

/**
 * Sign canonical bytes with the key derived from `sessionId` — the
 * end-to-end "intent tag" for a destructive op.
 */
export function signCanonical(
  platform: Platform,
  sessionId: Uint8Array,
  canonical: Uint8Array,
): string {
  return computeTagHex(platform, deriveSessionHmacKey(platform, sessionId), canonical);
}

// ── Per-op canonical inputs (mirror hmac.rs canonical_* fns) ──────────

export function canonicalDropDb(db: string): Uint8Array {
  return joinNull(['drop_db', db]);
}

export function canonicalDropRepo(dbInUse: string, repo: string): Uint8Array {
  return joinNull(['drop_repo', dbInUse, repo]);
}

export function canonicalDropTable(
  dbInUse: string,
  repo: string,
  table: string,
): Uint8Array {
  return joinNull(['drop_table', dbInUse, repo, table]);
}

export function canonicalDropIndex(
  dbInUse: string,
  repo: string,
  table: string,
  index: string,
  unique: boolean,
): Uint8Array {
  return joinNull([
    'drop_index',
    dbInUse,
    repo,
    table,
    index,
    unique ? '1' : '0',
  ]);
}

export function canonicalDropUser(username: string): Uint8Array {
  return joinNull(['drop_user', username]);
}

export function canonicalDropRole(role: string): Uint8Array {
  return joinNull(['drop_role', role]);
}

export function canonicalStartMigration(
  dbInUse: string,
  srcRepo: string,
  table: string,
  dstRepo: string,
  dstEngine: string,
): Uint8Array {
  return joinNull([
    'start_migration',
    dbInUse,
    srcRepo,
    table,
    dstRepo,
    dstEngine,
  ]);
}

export function canonicalCommitMigration(
  dbInUse: string,
  migrationId: string,
): Uint8Array {
  return joinNull(['commit_migration', dbInUse, migrationId]);
}

export function canonicalRollbackMigration(
  dbInUse: string,
  migrationId: string,
): Uint8Array {
  return joinNull(['rollback_migration', dbInUse, migrationId]);
}

export function canonicalGrantRole(role: string, user: string): Uint8Array {
  return joinNull(['grant_role', role, user]);
}

export function canonicalRevokeRole(role: string, user: string): Uint8Array {
  return joinNull(['revoke_role', role, user]);
}

export function canonicalCreateUser(username: string): Uint8Array {
  // Password is NEVER part of the canonical input — the tag confirms
  // "you meant to create this account", not the credential.
  return joinNull(['create_user', username]);
}

export function canonicalCreateRole(role: string): Uint8Array {
  // Permissions are not part of the canonical input, mirroring
  // `drop_role`'s precedent of identifying the op by name only.
  return joinNull(['create_role', role]);
}

/**
 * Wire-encodable reference to a securable resource — byte-for-byte mirror
 * of the Rust `ResourceRef` (untagged, single-key object). Only the
 * shape needed to compute a canonical string is declared here; the full
 * type lives in `./types/admin.ts`.
 */
type ResourceRefLike =
  | { database: string }
  | { store: [string, string] }
  | { table: [string, string, string] }
  | { function: string }
  | { function_folder: string[] }
  | { function_namespace: boolean };

/**
 * Render a `ResourceRef` into the stable `scheme://path` string used by
 * `canonicalChmod` / `canonicalChown` / `canonicalChgrp`. Mirrors the
 * Rust `canonical_resource_ref` in `hmac.rs` (itself matching
 * `ResourcePath`'s `Display` shape) byte-for-byte.
 */
export function canonicalResourceRef(r: ResourceRefLike): string {
  if ('database' in r) return `db://${r.database}`;
  if ('store' in r) return `db://${r.store[0]}/${r.store[1]}`;
  if ('table' in r) return `db://${r.table[0]}/${r.table[1]}/${r.table[2]}`;
  if ('function' in r) return `fn://${r.function}`;
  if ('function_folder' in r) return `fn://${r.function_folder.join('/')}/`;
  if ('function_namespace' in r) return 'fn://';
  // Compile-time exhaustiveness guard: a future `ResourceRefLike` member
  // that isn't handled above fails `tsc` here instead of silently
  // falling through to a wrong scheme (mirrors the Rust `canonical_resource_ref`
  // match, which has no wildcard arm for the same reason).
  const exhaustive: never = r;
  throw new Error(`unhandled ResourceRef variant: ${JSON.stringify(exhaustive)}`);
}

export function canonicalChmod(resource: ResourceRefLike, mode: number): Uint8Array {
  return joinNull(['chmod', canonicalResourceRef(resource), String(mode)]);
}

export function canonicalChown(
  resource: ResourceRefLike,
  owner: number | bigint,
): Uint8Array {
  return joinNull(['chown', canonicalResourceRef(resource), String(owner)]);
}

/**
 * `group: null` (clear the group) canonicalizes to the literal sentinel
 * `"null"` — mirrors the Rust `canonical_chgrp`.
 */
export function canonicalChgrp(
  resource: ResourceRefLike,
  group: number | bigint | null,
): Uint8Array {
  const groupStr = group === null ? 'null' : String(group);
  return joinNull(['chgrp', canonicalResourceRef(resource), groupStr]);
}

/**
 * Per-table history retention — byte-for-byte mirror of the Rust
 * `Retention` shape needed to compute the canonical string.
 */
interface RetentionLike {
  max_age_secs?: number;
  max_count?: number;
  min_count?: number;
}

/**
 * Render a `Retention` into the stable textual form used by
 * `canonicalSetRetention`. Mirrors the Rust `canonical_retention`:
 * each of the three orthogonal optional knobs rendered as its decimal
 * value or the sentinel `"none"`, comma-joined in field-declaration order.
 */
export function canonicalRetention(r: RetentionLike): string {
  const age = r.max_age_secs === undefined ? 'none' : String(r.max_age_secs);
  const max = r.max_count === undefined ? 'none' : String(r.max_count);
  const min = r.min_count === undefined ? 'none' : String(r.min_count);
  return `${age},${max},${min}`;
}

export function canonicalSetRetention(
  dbInUse: string,
  repo: string,
  table: string,
  retention: RetentionLike,
): Uint8Array {
  return joinNull([
    'set_retention',
    dbInUse,
    repo,
    table,
    canonicalRetention(retention),
  ]);
}

/**
 * Imperative history purge scope — byte-for-byte mirror of the Rust
 * `PurgeScope` shape needed to compute the canonical string.
 */
type PurgeScopeLike =
  | { older_than: { timestamp: number } }
  | { older_than_age: { age_secs: number } };

/**
 * Render a `PurgeScope` into the stable textual form used by
 * `canonicalPurgeHistory`. Mirrors the Rust `canonical_purge_scope`:
 * `"older_than:<timestamp>"` or `"older_than_age:<age_secs>"`.
 */
export function canonicalPurgeScope(scope: PurgeScopeLike): string {
  if ('older_than' in scope) return `older_than:${scope.older_than.timestamp}`;
  return `older_than_age:${scope.older_than_age.age_secs}`;
}

export function canonicalPurgeHistory(
  dbInUse: string,
  repo: string,
  table: string,
  scope: PurgeScopeLike,
): Uint8Array {
  return joinNull([
    'purge_history',
    dbInUse,
    repo,
    table,
    canonicalPurgeScope(scope),
  ]);
}
