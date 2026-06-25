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

  it('opts.returningFields emits select.fields projection', () => {
    const op = write.insert('users', { name: 'Alice', age: 30 }, {
      returningFields: ['name'],
    });
    expect(op.select).toEqual({ fields: ['name'] });
  });

  it('without returningFields omits select key', () => {
    const op = write.insert('users', { name: 'Alice' });
    expect(op).not.toHaveProperty('select');
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

  it('opts.returning emits empty select (all fields)', () => {
    const op = write.del('users', filter.eq('id', 42), { returning: true });
    expect(op.select).toEqual({});
    expect(op.select).not.toHaveProperty('fields');
  });

  it('opts.returningFields emits select.fields projection', () => {
    const op = write.del('users', filter.eq('id', 42), {
      returningFields: ['id', 'name'],
    });
    expect(op.select).toEqual({ fields: ['id', 'name'] });
  });

  it('returningFields takes precedence over returning', () => {
    const op = write.del('users', filter.eq('id', 42), {
      returning: true,
      returningFields: ['id'],
    });
    expect(op.select).toEqual({ fields: ['id'] });
  });

  it('without returning opts omits select key', () => {
    const op = write.del('users', filter.eq('id', 42));
    expect(op).not.toHaveProperty('select');
  });
});

// ── computed-write parity (B6) ──────────────────────────────────────

describe('computed-write parity (filter.* inside write values)', () => {
  it('insert accepts filter.fn(...) as a field value', () => {
    const op = write.insert('events', { created_at: filter.fn('NOW') });
    expect(op).toEqual({
      insert_into: 'events',
      values: [{ created_at: { $fn: 'NOW' } }],
    });
  });

  it('insert mixes literals and computed expressions', () => {
    const op = write.insert('orders', {
      id: 42,
      created_at: filter.fn('NOW'),
      total: filter.ref('price'),
    });
    expect(op).toEqual({
      insert_into: 'orders',
      values: [
        {
          id: 42,
          created_at: { $fn: 'NOW' },
          total: { $ref: ['price'] },
        },
      ],
    });
  });

  it('insert accepts filter.queryRef(...) as a field value', () => {
    const op = write.insert('carts', {
      owner: filter.queryRef('@users', '[0].id'),
    });
    expect(op).toEqual({
      insert_into: 'carts',
      values: [{ owner: { $query: '@users', path: '[0].id' } }],
    });
  });

  it('insert accepts filter.expr(...) as a field value', () => {
    const op = write.insert('rows', {
      gross: filter.expr('add', [filter.ref('net'), filter.ref('tax')]),
    });
    expect(op).toEqual({
      insert_into: 'rows',
      values: [
        {
          gross: {
            $expr: {
              op: 'add',
              args: [{ $ref: ['net'] }, { $ref: ['tax'] }],
            },
          },
        },
      ],
    });
  });

  it('insert accepts filter.cond(...) as a field value', () => {
    const op = write.insert('rows', {
      band: filter.cond(filter.gt('x', 10), 'hi', 'lo'),
    });
    expect(op).toEqual({
      insert_into: 'rows',
      values: [
        {
          band: {
            $cond: {
              if: { op: 'gt', field: ['x'], value: 10 },
              then: 'hi',
              else: 'lo',
            },
          },
        },
      ],
    });
  });

  it('insert accepts filter.param(...) as a field value', () => {
    const op = write.insert('rows', { label: filter.param('p1') });
    expect(op).toEqual({
      insert_into: 'rows',
      values: [{ label: { $param: 'p1' } }],
    });
  });

  it('insert accepts filter.fn(name, args) complex variant', () => {
    const op = write.insert('rows', {
      v: filter.fn('SUBSTRING', [filter.ref('s'), 0, 3]),
    });
    expect(op).toEqual({
      insert_into: 'rows',
      values: [
        {
          v: {
            $fn: {
              name: 'SUBSTRING',
              args: [{ $ref: ['s'] }, 0, 3],
            },
          },
        },
      ],
    });
  });

  it('update().set({...}) accepts a computed $ref value', () => {
    const op = write.update('orders')
      .where(filter.eq('id', 7))
      .set({ total: filter.ref('subtotal') })
      .build();
    expect(op).toEqual({
      update: 'orders',
      where: { op: 'eq', field: ['id'], value: 7 },
      set: { total: { $ref: ['subtotal'] } },
    });
  });

  it('update().set({...}) mixes literal and computed', () => {
    const op = write.update('orders')
      .set({ updated_at: filter.fn('NOW'), flagged: true })
      .build();
    expect(op.set).toEqual({
      updated_at: { $fn: 'NOW' },
      flagged: true,
    });
  });

  it('upsert value accepts a computed expression', () => {
    const op = write.upsert('events', { id: 1 }, {
      stamped: filter.fn('NOW'),
    });
    expect(op).toEqual({
      set: 'events',
      key: { id: 1 },
      value: { stamped: { $fn: 'NOW' } },
    });
  });

  it('upsert key accepts a computed $query ref', () => {
    const op = write.upsert('kv', filter.queryRef('@ins', '[0].id'), 'v');
    expect(op).toEqual({
      set: 'kv',
      key: { $query: '@ins', path: '[0].id' },
      value: 'v',
    });
  });

  // ── literal regression (computed-write extension must not break literals) ──

  it('insert literal values still produce plain wire shapes', () => {
    const op = write.insert('users', { name: 'Alice', age: 30, active: true });
    expect(op).toEqual({
      insert_into: 'users',
      values: [{ name: 'Alice', age: 30, active: true }],
    });
  });

  it('update literal .set({...}) still produces plain wire shapes', () => {
    const op = write.update('users').set({ age: 31 }).build();
    expect(op).toEqual({
      update: 'users',
      set: { age: 31 },
    });
  });

  it('upsert literal values still produce plain wire shapes', () => {
    const op = write.upsert('users', { id: 1 }, { name: 'Alice' });
    expect(op).toEqual({
      set: 'users',
      key: { id: 1 },
      value: { name: 'Alice' },
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
