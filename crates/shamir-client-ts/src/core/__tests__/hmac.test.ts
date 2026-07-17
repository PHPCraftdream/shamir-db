/**
 * HMAC canonical-input + tag byte-exactness tests.
 *
 * The authority is `crates/shamir-query-types/src/hmac.rs` (its #[test]
 * vectors are reproduced verbatim below) cross-checked against the e2e
 * reference `tests/e2e/helpers/hmac.js` (node:crypto). If these pass, the
 * TS client produces tags the Rust server accepts byte-for-byte.
 */

import { describe, it, expect } from 'vitest';
import { createHash, createHmac } from 'node:crypto';
import {
  joinNull,
  deriveSessionHmacKey,
  signCanonical,
  canonicalDropDb,
  canonicalDropRepo,
  canonicalDropTable,
  canonicalDropIndex,
  canonicalDropUser,
  canonicalSetSuperuser,
  canonicalStartMigration,
  canonicalCommitMigration,
  canonicalRollbackMigration,
  canonicalCreateFunction,
  canonicalCreateScramUser,
} from '../hmac.js';
import { NodePlatform } from '../../platform/node.js';

const enc = new TextEncoder();
/** Expected canonical bytes from a JS string with embedded NULs. */
function b(s: string): number[] {
  return Array.from(enc.encode(s));
}
function arr(u: Uint8Array): number[] {
  return Array.from(u);
}

describe('canonical inputs are null-separated (hmac.rs vectors)', () => {
  it('drop_db', () => {
    expect(arr(canonicalDropDb('mydb'))).toEqual(b('drop_db\0mydb'));
  });
  it('drop_repo', () => {
    expect(arr(canonicalDropRepo('mydb', 'cold'))).toEqual(
      b('drop_repo\0mydb\0cold'),
    );
  });
  it('drop_table', () => {
    expect(arr(canonicalDropTable('mydb', 'main', 'users'))).toEqual(
      b('drop_table\0mydb\0main\0users'),
    );
  });
  it('drop_index (unique=false → trailing "0")', () => {
    expect(
      arr(canonicalDropIndex('mydb', 'main', 'users', 'by_email', false)),
    ).toEqual(b('drop_index\0mydb\0main\0users\0by_email\x000'));
  });
  it('drop_index (unique=true → trailing "1")', () => {
    expect(
      arr(canonicalDropIndex('mydb', 'main', 'users', 'by_email', true)),
    ).toEqual(b('drop_index\0mydb\0main\0users\0by_email\x001'));
  });
  it('drop_user', () => {
    expect(arr(canonicalDropUser('bob'))).toEqual(b('drop_user\0bob'));
  });
  it('set_superuser — on=true renders literal "true"', () => {
    expect(arr(canonicalSetSuperuser('carol', true))).toEqual(
      b('set_superuser\0carol\0true'),
    );
  });
  it('set_superuser — on=false renders literal "false"', () => {
    expect(arr(canonicalSetSuperuser('dave', false))).toEqual(
      b('set_superuser\0dave\0false'),
    );
  });
  it('migration canonicals', () => {
    expect(
      arr(canonicalStartMigration('mydb', 'main', 'users', 'cold', 'redb')),
    ).toEqual(b('start_migration\0mydb\0main\0users\0cold\0redb'));
    expect(arr(canonicalCommitMigration('mydb', 'mig-001'))).toEqual(
      b('commit_migration\0mydb\0mig-001'),
    );
    expect(arr(canonicalRollbackMigration('mydb', 'mig-001'))).toEqual(
      b('rollback_migration\0mydb\0mig-001'),
    );
  });
  it('create_function — definer, no grants (csv = empty)', () => {
    expect(arr(canonicalCreateFunction('my_fn', 'definer', []))).toEqual(
      b('create_function\0my_fn\0definer\0'),
    );
  });
  it('create_function — invoker, grants joined by comma in given order', () => {
    expect(
      arr(canonicalCreateFunction('my_fn', 'invoker', ['FOO', 'BAR'])),
    ).toEqual(b('create_function\0my_fn\0invoker\0FOO,BAR'));
  });
  it('create_function — grants are NOT sorted/deduped (caller order preserved)', () => {
    expect(
      arr(canonicalCreateFunction('f', 'invoker', ['B', 'A', 'B'])),
    ).toEqual(b('create_function\0f\0invoker\0B,A,B'));
  });
  // Regression coverage for #634's real root cause: `createScramUser` never
  // sent the `hmac` field the server has required since task #604 (the
  // Rust `shamir-client` crate was updated; `shamir-client-ts` was not),
  // which silently broke every e2e test that provisions a user via
  // `createScramUser` (cascading into a `resolvedId` left `undefined` on
  // the wire — msgpack-encoded as `nil`, rejected by the server as
  // "invalid type: unit value, expected u64" on the *next* op). This byte
  // vector mirrors `canonical_create_scram_user` in `hmac.rs` so a missing
  // or wrong canonical for this op is caught here, not only in an e2e run.
  it('create_scram_user — no roles', () => {
    expect(arr(canonicalCreateScramUser('bob', []))).toEqual(
      b('create_scram_user\0bob'),
    );
  });
  it('create_scram_user — roles joined in given order (NOT sorted/deduped)', () => {
    expect(
      arr(canonicalCreateScramUser('alice', ['reader', 'writer'])),
    ).toEqual(b('create_scram_user\0alice\0reader\0writer'));
  });
});

describe('joinNull edge cases', () => {
  it('single part has no NUL', () => {
    expect(arr(joinNull(['x']))).toEqual(b('x'));
  });
  it('no leading/trailing NUL between three parts', () => {
    expect(arr(joinNull(['a', 'b', 'c']))).toEqual(b('a\0b\0c'));
  });
});

describe('key derivation matches node:crypto (e2e reference)', () => {
  it('SHA256("shamir-db hmac key v1\\0" || session_id)', () => {
    const sid = new Uint8Array(32).fill(7);
    const expected = createHash('sha256')
      .update('shamir-db hmac key v1\0', 'utf8')
      .update(sid)
      .digest();
    expect(arr(deriveSessionHmacKey(NodePlatform, sid))).toEqual(
      Array.from(expected),
    );
  });

  it('domain-separated + deterministic', () => {
    const sid = new Uint8Array(32).fill(7);
    const k1 = deriveSessionHmacKey(NodePlatform, sid);
    const k2 = deriveSessionHmacKey(NodePlatform, sid);
    expect(arr(k1)).toEqual(arr(k2));
    expect(arr(k1)).not.toEqual(arr(sid)); // not the raw session_id
    const other = new Uint8Array(32).fill(7);
    other[0] ^= 0xff;
    expect(arr(deriveSessionHmacKey(NodePlatform, other))).not.toEqual(arr(k1));
  });
});

describe('full sign matches node:crypto end-to-end', () => {
  it('signCanonical == hex(HMAC-SHA256(deriveKey(sid), canonical))', () => {
    const sid = new Uint8Array(32).fill(1);
    const canonical = canonicalDropTable('db', 'main', 'users');

    const key = createHash('sha256')
      .update('shamir-db hmac key v1\0', 'utf8')
      .update(sid)
      .digest();
    const expectedTag = createHmac('sha256', key)
      .update(Buffer.from(canonical))
      .digest('hex');

    expect(signCanonical(NodePlatform, sid, canonical)).toBe(expectedTag);
  });

  it('different op bytes → different tag', () => {
    const sid = new Uint8Array(32).fill(1);
    const a = signCanonical(NodePlatform, sid, canonicalDropTable('db', 'main', 'users'));
    const c = signCanonical(NodePlatform, sid, canonicalDropTable('db', 'main', 'OTHER'));
    expect(a).not.toBe(c);
  });
});
