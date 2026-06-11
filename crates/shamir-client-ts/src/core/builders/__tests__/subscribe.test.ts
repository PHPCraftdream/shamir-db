/**
 * Subscribe-builder wire-shape tests.
 */

import { describe, it, expect } from 'vitest';
import { subscribe, unsubscribeOp } from '../subscribe.js';
import { eq, exists } from '../filter.js';
import { Batch } from '../batch.js';
import { Query } from '../query.js';

describe('subscribe', () => {
  it('single source with filter callback → correct wire shape', () => {
    const op = subscribe({
      store: 'main',
      table: 'messages',
      where: (f) => f.eq('thread_id', 42),
    });
    expect(op).toEqual({
      subscribe: [
        {
          table: 'messages',
          filter: { op: 'eq', field: ['thread_id'], value: 42 },
        },
      ],
    });
  });

  it('single source with filter literal → same wire shape', () => {
    const op = subscribe({
      store: 'main',
      table: 'messages',
      where: eq('thread_id', 42),
    });
    expect(op).toEqual({
      subscribe: [
        {
          table: 'messages',
          filter: { op: 'eq', field: ['thread_id'], value: 42 },
        },
      ],
    });
  });

  it('non-main repo emits [repo, table] tuple', () => {
    const op = subscribe({
      store: 'hot',
      table: 'sessions',
      where: eq('x', 1),
    });
    expect(op.subscribe[0].table).toEqual(['hot', 'sessions']);
  });

  it('multiple sources in one subscribe', () => {
    const op = subscribe([
      { store: 'main', table: 'messages', where: eq('x', 1) },
      { store: 'main', table: 'users', where: exists('online') },
    ]);
    expect(op.subscribe).toHaveLength(2);
    expect(op.subscribe[0].table).toBe('messages');
    expect(op.subscribe[1].table).toBe('users');
  });

  it('on: ["put"] maps to EventMask "put"', () => {
    const op = subscribe({
      store: 'main',
      table: 't',
      where: eq('a', 1),
      on: ['put'],
    });
    expect(op.subscribe[0].events).toBe('put');
  });

  it('on: ["any"] maps to EventMask "all"', () => {
    const op = subscribe({
      store: 'main',
      table: 't',
      where: eq('a', 1),
      on: ['any'],
    });
    expect(op.subscribe[0].events).toBe('all');
  });

  it('on: ["put", "delete"] maps to "all"', () => {
    const op = subscribe({
      store: 'main',
      table: 't',
      where: eq('a', 1),
      on: ['put', 'delete'],
    });
    expect(op.subscribe[0].events).toBe('all');
  });

  it('handle callback produces externally-tagged batch deliver', () => {
    const op = subscribe({
      store: 'main',
      table: 'messages',
      where: eq('x', 1),
      handle: (b) => b.add('inner', Query.from('threads')),
    });
    expect(op.deliver).toBeDefined();
    const d = op.deliver as { batch: { batch: object } };
    expect(d.batch).toBeDefined();
    expect(d.batch.batch).toHaveProperty('queries');
  });

  it('handle with bind threads bindings', () => {
    const op = subscribe({
      store: 'main',
      table: 'messages',
      where: eq('x', 1),
      handle: (b) => b.add('inner', Query.from('threads')),
      bind: { tid: 42 },
    });
    const d = op.deliver as { batch: { batch: object; bind: Record<string, unknown> } };
    expect(d.batch.bind).toEqual({ tid: 42 });
  });

  it('deliver: "keys" produces bare string "keys"', () => {
    const op = subscribe({
      store: 'main',
      table: 'users',
      where: exists('online'),
      deliver: 'keys',
    });
    expect(op.deliver).toBe('keys');
  });

  it('no filter omits filter field', () => {
    const op = subscribe({ store: 'main', table: 't' });
    expect(op.subscribe[0].filter).toBeUndefined();
  });

  it('initial: true flag propagates', () => {
    const op = subscribe(
      { store: 'main', table: 't', where: eq('a', 1) },
      { initial: true },
    );
    expect(op.initial).toBe(true);
  });

  it('throws when sources disagree on deliver mode', () => {
    expect(() =>
      subscribe([
        { store: 'main', table: 'a', deliver: 'keys' },
        { store: 'main', table: 'b', deliver: 'records' },
      ]),
    ).toThrow(/conflicting deliver/);
  });

  it('accepts multiple sources that agree on deliver mode', () => {
    const op = subscribe([
      { store: 'main', table: 'a', deliver: 'keys' },
      { store: 'main', table: 'b', deliver: 'keys' },
    ]);
    expect(op.deliver).toBe('keys');
    expect(op.subscribe).toHaveLength(2);
  });

  it('fromVersion propagates as from_version', () => {
    const op = subscribe(
      { store: 'main', table: 't', where: eq('a', 1) },
      { fromVersion: 99 },
    );
    expect(op.from_version).toBe(99);
  });
});

describe('unsubscribeOp', () => {
  it('produces { unsubscribe: 42 }', () => {
    expect(unsubscribeOp(42)).toEqual({ unsubscribe: 42 });
  });
});
