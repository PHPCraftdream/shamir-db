/**
 * Unit tests for the fxhash64 principal-id replica.
 *
 * The canonical source of truth is `fxhash::hash64(username) & (i64::MAX as u64)`
 * computed by the Rust server (`crates/shamir-types/src/access.rs::principal_id`).
 *
 * These tests verify internal consistency (determinism, bigint type,
 * i64 range). The definitive cross-language match is proven in the
 * e2e-principal test, which compares the TS hash against the running
 * server's access_tree principal id.
 */

import { describe, it, expect } from 'vitest';
import { principalId } from '../principal-id.js';

describe('principalId', () => {
  it('returns a bigint', () => {
    expect(typeof principalId('admin')).toBe('bigint');
  });

  it('is deterministic', () => {
    expect(principalId('alice')).toBe(principalId('alice'));
    expect(principalId('bob')).toBe(principalId('bob'));
  });

  it('different usernames produce different ids', () => {
    expect(principalId('alice')).not.toBe(principalId('bob'));
    expect(principalId('admin')).not.toBe(principalId('user'));
  });

  it('result is masked to i64::MAX (63 bits)', () => {
    const I64MAX = 0x7FFFFFFFFFFFFFFFn;
    for (const name of ['admin', 'alice', 'bob', 'test_user_123', '']) {
      const id = principalId(name);
      expect(id).toBeGreaterThanOrEqual(0n);
      expect(id).toBeLessThanOrEqual(I64MAX);
    }
  });

  it('empty string has a defined hash', () => {
    const id = principalId('');
    expect(typeof id).toBe('bigint');
    expect(id).toBeGreaterThanOrEqual(0n);
  });

  it('handles multi-byte UTF-8 (non-ASCII usernames)', () => {
    const id = principalId('\u{1F600}'); // emoji
    expect(typeof id).toBe('bigint');
    expect(id).toBeGreaterThanOrEqual(0n);
    expect(id).toBeLessThanOrEqual(0x7FFFFFFFFFFFFFFFn);
  });

  it('handles strings that exercise all chunk sizes', () => {
    // 1 byte -> 1-byte path only + terminator
    const id1 = principalId('a');
    // 2 bytes -> 2-byte path + terminator
    const id2 = principalId('ab');
    // 3 bytes -> 2-byte + 1-byte path + terminator
    const id3 = principalId('abc');
    // 4 bytes -> 4-byte path + terminator
    const id4 = principalId('abcd');
    // 7 bytes -> 4+2+1 path + terminator
    const id7 = principalId('abcdefg');
    // 8 bytes -> 8-byte path + terminator
    const id8 = principalId('abcdefgh');
    // 15 bytes -> 8+4+2+1 + terminator
    const id15 = principalId('abcdefghijklmno');

    // All distinct
    const ids = [id1, id2, id3, id4, id7, id8, id15];
    const unique = new Set(ids);
    expect(unique.size).toBe(ids.length);
  });
});
