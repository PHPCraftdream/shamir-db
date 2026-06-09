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
