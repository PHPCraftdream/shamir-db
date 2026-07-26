/**
 * Unit tests for `QueryResult.corrupt_records` (F-22, #815).
 *
 * `corrupt_records` mirrors `query_result.rs::QueryResult.corrupt_records`
 * (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`) — omitted
 * from the wire when empty. These tests feed a hand-built msgpack payload
 * (mirroring the server's wire shape) through the real decode path
 * (`encode`/`decode` from `framing.ts`) and assert the typed
 * `CorruptRecordRef[]` shape comes back, AND that the common case (field
 * omitted from the wire) leaves it `undefined` — a regression guard
 * against a false-positive default.
 */

import { describe, it, expect } from 'vitest';
import { encode, decode } from '../framing.js';
import type { QueryResult, CorruptRecordRef } from '../types/batch.js';

describe('QueryResult.corrupt_records decode shape', () => {
  it('decodes a response containing corrupt_records into typed CorruptRecordRef[]', () => {
    // Mirrors the server's wire shape: CorruptRecordRef.id is a base58
    // STRING (F-22 fix), not raw bytes.
    const wire = {
      records: [],
      corrupt_records: [
        { table: 'widgets', id: '2vY8Kx9pQmN3rT7wJhLd4b' },
        { table: 'orders', id: '5zF1Wc2eRkP8sN0qXjMh6a' },
      ],
    };

    const decoded = decode(encode(wire)) as QueryResult;

    expect(decoded.corrupt_records).toBeDefined();
    expect(decoded.corrupt_records).toHaveLength(2);

    const [first, second] = decoded.corrupt_records as CorruptRecordRef[];
    expect(first.table).toBe('widgets');
    expect(first.id).toBe('2vY8Kx9pQmN3rT7wJhLd4b');
    expect(typeof first.id).toBe('string');
    expect(second.table).toBe('orders');
    expect(second.id).toBe('5zF1Wc2eRkP8sN0qXjMh6a');
  });

  it('leaves corrupt_records undefined when omitted from the wire (common case)', () => {
    // The common case: nothing corrupt, so the server omits the field
    // entirely (skip_serializing_if = "Vec::is_empty") rather than sending
    // an empty array. A false-positive default (e.g. `[]`) would hide this
    // regression.
    const wire = {
      records: [{ name: 'widget' }],
    };

    const decoded = decode(encode(wire)) as QueryResult;

    expect(decoded.corrupt_records).toBeUndefined();
  });
});
