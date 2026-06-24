/**
 * Unit tests for interner-ops: encodeRecordIdMsgpack, qvHasFnMarker,
 * collectFieldNames, deinternResponse.
 */

import { describe, it, expect } from 'vitest';
import { FieldMap, InternerCacheRegistry } from '../field-map.js';
import {
  encodeRecordIdMsgpack,
  qvHasFnMarker,
  collectFieldNames,
  deinternResponse,
} from '../interner-ops.js';
import type { BatchResponse } from '../types/batch.js';

// ── helpers ──────────────────────────────────────────────────────────────────

/** Build a FieldMap with pre-populated entries. */
function buildFieldMap(entries: [string, bigint][]): FieldMap {
  const fm = new FieldMap();
  for (const [name, id] of entries) {
    fm.insertEntry(name, id);
  }
  return fm;
}

/**
 * Parse a bin8 key from bytes at position, returning the LE id and new pos.
 * Expects 0xc4 marker.
 */
function readBinKey(bytes: Uint8Array, pos: number): { id: number; end: number } {
  expect(bytes[pos]).toBe(0xc4); // bin8
  const len = bytes[pos + 1];
  const payload = bytes.subarray(pos + 2, pos + 2 + len);
  let id = 0;
  for (let i = 0; i < len; i++) {
    id |= payload[i] << (i * 8);
  }
  return { id, end: pos + 2 + len };
}

// ── encodeRecordIdMsgpack ───────────────────────────────────────────────────

describe('encodeRecordIdMsgpack', () => {
  it('encodes a flat record with id-keyed bin keys', () => {
    const fm = buildFieldMap([
      ['name', 1n],
      ['age', 2n],
    ]);
    const record = { name: 'Alice', age: 30 };
    const bytes = encodeRecordIdMsgpack(record, fm);

    // Should start with a fixmap header for 2 entries.
    expect(bytes[0]).toBe(0x82); // fixmap(2)

    // Parse the two entries manually.
    const key1 = readBinKey(bytes, 1);
    expect(key1.id).toBe(1); // name -> id 1

    const key2Start = findNextBinKey(bytes, key1.end);
    const key2 = readBinKey(bytes, key2Start);
    expect(key2.id).toBe(2); // age -> id 2
  });

  it('round-trips through encode + deintern', () => {
    const fm = buildFieldMap([
      ['name', 1n],
      ['score', 2n],
    ]);
    const record = { name: 'Bob', score: 99 };
    const encoded = encodeRecordIdMsgpack(record, fm);

    // De-intern: wrap in a fake response and deintern.
    const registry = new InternerCacheRegistry();
    const regFm = registry.getOrCreate('db', 'main');
    regFm.insertEntry('name', 1n);
    regFm.insertEntry('score', 2n);

    const response = {
      id: 1,
      results: {
        q: { records: [encoded] },
      },
      execution_plan: [['q']],
      execution_time_us: 0,
    } as unknown as BatchResponse;

    const result = deinternResponse(registry, 'db', response, ['main']);
    const rec = result.results['q'].records[0] as Record<string, unknown>;
    expect(rec['name']).toBe('Bob');
    expect(rec['score']).toBe(99);
  });

  it('encodes nested map keys recursively', () => {
    const fm = buildFieldMap([
      ['profile', 1n],
      ['age', 2n],
      ['city', 3n],
    ]);
    const record = {
      profile: {
        age: 25,
        city: 'Berlin',
      },
    };
    const encoded = encodeRecordIdMsgpack(record, fm);

    // Round-trip via deintern.
    const registry = new InternerCacheRegistry();
    const regFm = registry.getOrCreate('db', 'main');
    regFm.insertEntry('profile', 1n);
    regFm.insertEntry('age', 2n);
    regFm.insertEntry('city', 3n);

    const response = {
      id: 1,
      results: { q: { records: [encoded] } },
      execution_plan: [['q']],
      execution_time_us: 0,
    } as unknown as BatchResponse;

    const result = deinternResponse(registry, 'db', response, ['main']);
    const rec = result.results['q'].records[0] as Record<string, unknown>;
    const profile = rec['profile'] as Record<string, unknown>;
    expect(profile['age']).toBe(25);
    expect(profile['city']).toBe('Berlin');
  });

  it('uses minimal LE encoding for ids > 255', () => {
    const fm = buildFieldMap([['field_a', 300n]]);
    const record = { field_a: 'value' };
    const bytes = encodeRecordIdMsgpack(record, fm);

    // Map header + bin8 key.
    expect(bytes[0]).toBe(0x81); // fixmap(1)
    expect(bytes[1]).toBe(0xc4); // bin8
    expect(bytes[2]).toBe(2);    // key length = 2
    // id=300 -> LE: [0x2C, 0x01]
    expect(bytes[3]).toBe(300 & 0xFF);       // 0x2C = 44
    expect(bytes[4]).toBe((300 >> 8) & 0xFF); // 0x01
  });

  it('throws when a field name is not in the FieldMap', () => {
    const fm = buildFieldMap([['known', 1n]]);
    const record = { known: 'ok', unknown: 'fail' };
    expect(() => encodeRecordIdMsgpack(record, fm)).toThrow(
      "field 'unknown' not in FieldMap",
    );
  });

  it('handles arrays in values without interning array elements', () => {
    const fm = buildFieldMap([['tags', 1n]]);
    const record = { tags: ['a', 'b', 'c'] };
    const encoded = encodeRecordIdMsgpack(record, fm);

    // Round-trip.
    const registry = new InternerCacheRegistry();
    registry.getOrCreate('db', 'main').insertEntry('tags', 1n);
    const response = {
      id: 1,
      results: { q: { records: [encoded] } },
      execution_plan: [['q']],
      execution_time_us: 0,
    } as unknown as BatchResponse;
    const result = deinternResponse(registry, 'db', response, ['main']);
    const rec = result.results['q'].records[0] as Record<string, unknown>;
    expect(rec['tags']).toEqual(['a', 'b', 'c']);
  });

  it('handles null values', () => {
    const fm = buildFieldMap([['field', 1n]]);
    const record = { field: null };
    const encoded = encodeRecordIdMsgpack(record, fm);

    const registry = new InternerCacheRegistry();
    registry.getOrCreate('db', 'main').insertEntry('field', 1n);
    const response = {
      id: 1,
      results: { q: { records: [encoded] } },
      execution_plan: [['q']],
      execution_time_us: 0,
    } as unknown as BatchResponse;
    const result = deinternResponse(registry, 'db', response, ['main']);
    const rec = result.results['q'].records[0] as Record<string, unknown>;
    expect(rec['field']).toBeNull();
  });
});

// ── qvHasFnMarker ───────────────────────────────────────────────────────────

describe('qvHasFnMarker', () => {
  it('returns false for plain objects', () => {
    expect(qvHasFnMarker({ name: 'Alice', age: 30 })).toBe(false);
  });

  it('returns true for top-level $fn', () => {
    expect(qvHasFnMarker({ $fn: 'now', args: [] })).toBe(true);
  });

  it('returns true for nested $fn', () => {
    expect(
      qvHasFnMarker({
        profile: { $fn: 'compute', args: [1] },
      }),
    ).toBe(true);
  });

  it('returns true for $fn inside array', () => {
    expect(qvHasFnMarker([{ $fn: 'gen_id' }])).toBe(true);
  });

  it('returns false for null', () => {
    expect(qvHasFnMarker(null)).toBe(false);
  });

  it('returns false for scalars', () => {
    expect(qvHasFnMarker(42)).toBe(false);
    expect(qvHasFnMarker('hello')).toBe(false);
    expect(qvHasFnMarker(true)).toBe(false);
  });
});

// ── collectFieldNames ───────────────────────────────────────────────────────

describe('collectFieldNames', () => {
  it('collects field names from INSERT values', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      {
        insert_into: 'users',
        values: [
          { name: 'Alice', age: 30 },
          { name: 'Bob', score: 100 },
        ],
      },
      out,
    );
    const names = out.get('main')!;
    expect(names).toContain('name');
    expect(names).toContain('age');
    expect(names).toContain('score');
  });

  it('collects nested map keys recursively', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      {
        insert_into: 'users',
        values: [{ profile: { city: 'NYC' } }],
      },
      out,
    );
    const names = out.get('main')!;
    expect(names).toContain('profile');
    expect(names).toContain('city');
  });

  it('extracts repo from tuple table-ref', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      {
        insert_into: ['custom_repo', 'users'],
        values: [{ id: 1 }],
      },
      out,
    );
    expect(out.has('custom_repo')).toBe(true);
    expect(out.get('custom_repo')!).toContain('id');
  });

  it('collects from SET (upsert) key and value', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      {
        set: 'users',
        key: { user_id: 'u1' },
        value: { name: 'Alice', email: 'a@b.c' },
      },
      out,
    );
    const names = out.get('main')!;
    expect(names).toContain('user_id');
    expect(names).toContain('name');
    expect(names).toContain('email');
  });

  it('collects from UPDATE set', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      {
        update: 'users',
        set: { score: 42 },
      },
      out,
    );
    const names = out.get('main')!;
    expect(names).toContain('score');
  });

  it('ignores read ops', () => {
    const out = new Map<string, string[]>();
    collectFieldNames(
      { from: 'users', repo: 'main' },
      out,
    );
    expect(out.size).toBe(0);
  });
});

// ── deinternResponse ────────────────────────────────────────────────────────

describe('deinternResponse', () => {
  it('de-interns id-keyed records back to name-keyed objects', () => {
    const registry = new InternerCacheRegistry();
    const fm = registry.getOrCreate('testdb', 'main');
    fm.insertEntry('name', 1n);
    fm.insertEntry('age', 2n);

    // Encode a record as id-keyed msgpack.
    const encodeFm = buildFieldMap([
      ['name', 1n],
      ['age', 2n],
    ]);
    const idBytes = encodeRecordIdMsgpack({ name: 'Alice', age: 30 }, encodeFm);

    const response = {
      id: 1,
      results: {
        q: {
          records: [idBytes] as unknown[],
        },
      },
      execution_plan: [['q']],
      execution_time_us: 100,
    } as unknown as BatchResponse;

    const deinterned = deinternResponse(registry, 'testdb', response, ['main']);
    const rec = deinterned.results['q'].records[0] as Record<string, unknown>;
    expect(rec['name']).toBe('Alice');
    expect(rec['age']).toBe(30);
  });

  it('passes through non-id-keyed records unchanged', () => {
    const registry = new InternerCacheRegistry();
    const response = {
      id: 1,
      results: {
        q: {
          records: [{ name: 'Bob', age: 25 }],
        },
      },
      execution_plan: [['q']],
      execution_time_us: 50,
    } as unknown as BatchResponse;

    const result = deinternResponse(registry, 'testdb', response, ['main']);
    const rec = result.results['q'].records[0] as Record<string, unknown>;
    expect(rec['name']).toBe('Bob');
    expect(rec['age']).toBe(25);
  });
});

// ── helper ──────────────────────────────────────────────────────────────────

/**
 * Scan bytes from `start` to find the next bin8 key marker (0xc4),
 * skipping over encoded values.
 */
function findNextBinKey(bytes: Uint8Array, start: number): number {
  // Simple scan: find next 0xc4 that looks like a bin key.
  for (let i = start; i < bytes.length - 1; i++) {
    if (bytes[i] === 0xc4 && bytes[i + 1] >= 1 && bytes[i + 1] <= 8) {
      return i;
    }
  }
  throw new Error('no bin key found');
}
