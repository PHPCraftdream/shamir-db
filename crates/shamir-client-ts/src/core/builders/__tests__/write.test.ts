/**
 * Write-builder wire-shape tests.
 *
 * The authority for every shape is `crates/shamir-query-types/src/write/types.rs`
 * (serde: skip_serializing_if, rename = "where", rename_all = "lowercase",
 * default values) cross-checked with `table_refilter.rs` for TableRef serialisation.
 */

import { describe, it, expect } from 'vitest';
import { write } from '../write.js';
import { filter } from '../filter.js';

// ── insert ───────────────────────────────────────────────────────────

describe('insert', () => {
  it('single record → array of one, default repo → bare string', () => {
    const op = write.insert('users', { name: 'Alice', age: 30 });
    expect(op).toEqual({
      insert_into: 'users',
      values: [{ name: 'Alice', age: 30 }],
    });
  });

  it('array of records, default repo → bare string', () => {
    const op = write.insert('users', [
      { name: 'Alice' },
      { name: 'Bob' },
    ]);
    expect(op).toEqual({
      insert_into: 'users',
      values: [{ name: 'Alice' }, { name: 'Bob' }],
    });
  });

  it('explicit repo → [repo, table] tuple', () => {
    const op = write.insert('sessions', { id: 1 }, { repo: 'hot' });
    expect(op).toEqual({
      insert_into: ['hot', 'sessions'],
      values: [{ id: 1 }],
    });
  });

  it('repo "main" collapses to bare string', () => {
    const op = write.insert('users', { id: 1 }, { repo: 'main' });
    expect(op.insert_into).toBe('users');
  });
});

// ── update ───────────────────────────────────────────────────────────

describe('update', () => {
  it('.where().set() emits {update, where, set}; no select', () => {
    const op = write.update('users')
      .where(filter.eq('age', 30))
      .set({ age: 31 })
      .build();
    expect(op).toEqual({
      update: 'users',
      where: { op: 'eq', field: ['age'], value: 30 },
      set: { age: 31 },
    });
    expect(op).not.toHaveProperty('select');
  });

  it('without .where() omits where key', () => {
    const op = write.update('users').set({ active: true }).build();
    expect(op).toEqual({
      update: 'users',
      set: { active: true },
    });
    expect(op).not.toHaveProperty('where');
  });

  it('.returning("all", ["a"]) emits select with return_mode + fields', () => {
    const op = write.update('users')
      .set({ x: 1 })
      .returning('all', ['a'])
      .build();
    expect(op.select).toEqual({
      return_mode: 'all',
      fields: ['a'],
    });
  });

  it('.returning() with no args → select:{return_mode:"changed"} (always present)', () => {
    const op = write.update('users').set({ x: 1 }).returning().build();
    expect(op.select).toEqual({ return_mode: 'changed' });
    expect(op.select).not.toHaveProperty('fields');
  });

  it('.returning("unchanged") → return_mode:"unchanged"', () => {
    const op = write.update('users').set({ x: 1 }).returning('unchanged').build();
    expect(op.select).toEqual({ return_mode: 'unchanged' });
  });

  it('explicit repo → [repo, table] tuple', () => {
    const op = write.update('sessions', { repo: 'hot' }).set({ v: 2 }).build();
    expect(op.update).toEqual(['hot', 'sessions']);
  });

  it('throws on build without .set()', () => {
    expect(() => write.update('users').build()).toThrow(
      'update builder requires .set() before .build()',
    );
  });
});

// ── upsert (set) ─────────────────────────────────────────────────────

describe('upsert', () => {
  it('emits {set, key, value}', () => {
    const op = write.upsert('users', { id: 1 }, { name: 'Alice' });
    expect(op).toEqual({
      set: 'users',
      key: { id: 1 },
      value: { name: 'Alice' },
    });
  });

  it('explicit repo → tuple', () => {
    const op = write.upsert('kv', 'my-key', 'my-value', { repo: 'cache' });
    expect(op).toEqual({
      set: ['cache', 'kv'],
      key: 'my-key',
      value: 'my-value',
    });
  });
});

// ── del (delete) ─────────────────────────────────────────────────────

describe('del', () => {
  it('emits {delete_from, where}; where is always present', () => {
    const op = write.del('users', filter.eq('id', 42));
    expect(op).toEqual({
      delete_from: 'users',
      where: { op: 'eq', field: ['id'], value: 42 },
    });
  });

  it('explicit repo → tuple', () => {
    const op = write.del('sessions', filter.eq('token', 'abc'), { repo: 'hot' });
    expect(op.delete_from).toEqual(['hot', 'sessions']);
  });

  it('accepts complex filter (and / or)', () => {
    const op = write.del('users', filter.and(filter.eq('age', 30), filter.eq('status', 'inactive')));
    expect(op.where).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['age'], value: 30 },
        { op: 'eq', field: ['status'], value: 'inactive' },
      ],
    });
  });
});

// ── UpdateReturnMode values are lowercase strings ────────────────────

describe('UpdateReturnMode values', () => {
  it('all three modes are lowercase strings', () => {
    const modes: Array<import('../../types/write.js').UpdateReturnMode> = [
      'all',
      'changed',
      'unchanged',
    ];
    for (const m of modes) {
      expect(typeof m).toBe('string');
      expect(m).toBe(m.toLowerCase());
    }
  });
});
