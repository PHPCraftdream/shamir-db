/**
 * Select-item constructor wire-shape tests.
 *
 * Covers every exported constructor in `../select.ts`.
 */

import { describe, it, expect } from 'vitest';
import {
  all,
  field,
  countAll,
  aggregate,
  count,
  sum,
  avg,
  min,
  max,
  aggregateFn,
  func,
  select,
} from '../select.js';

describe('all', () => {
  it('returns { type: "all" }', () => {
    expect(all()).toEqual({ type: 'all' });
  });
});

describe('field', () => {
  it('string spec normalises to path array', () => {
    const item = field('x');
    expect(item).toEqual({ type: 'field', path: ['x'] });
    expect(item).not.toHaveProperty('alias');
  });

  it('array spec is kept as-is', () => {
    const item = field(['a', 'b']);
    expect(item).toEqual({ type: 'field', path: ['a', 'b'] });
    expect(item).not.toHaveProperty('alias');
  });

  it('with alias adds the alias key', () => {
    expect(field('x', 'xx')).toEqual({ type: 'field', path: ['x'], alias: 'xx' });
  });
});

describe('countAll', () => {
  it('returns { type: "count_all" } without alias', () => {
    const item = countAll();
    expect(item).toEqual({ type: 'count_all' });
    expect(item).not.toHaveProperty('alias');
  });

  it('with alias adds the alias key', () => {
    expect(countAll('n')).toEqual({ type: 'count_all', alias: 'n' });
  });
});

describe('aggregate', () => {
  it('string field normalises to path array; distinct defaults false', () => {
    const item = aggregate('sum', 'amount');
    expect(item).toEqual({
      type: 'aggregate',
      func: 'sum',
      field: ['amount'],
      distinct: false,
    });
    expect(item).not.toHaveProperty('alias');
  });

  it('array field is kept as-is', () => {
    expect(aggregate('avg', ['a', 'b'])).toEqual({
      type: 'aggregate',
      func: 'avg',
      field: ['a', 'b'],
      distinct: false,
    });
  });

  it('null field stays null (the * case)', () => {
    expect(aggregate('count', null)).toEqual({
      type: 'aggregate',
      func: 'count',
      field: null,
      distinct: false,
    });
  });

  it('distinct:true overrides the default', () => {
    expect(aggregate('sum', 'x', { distinct: true })).toEqual({
      type: 'aggregate',
      func: 'sum',
      field: ['x'],
      distinct: true,
    });
  });

  it('alias is added when provided', () => {
    expect(aggregate('max', 'score', { alias: 'best' })).toEqual({
      type: 'aggregate',
      func: 'max',
      field: ['score'],
      distinct: false,
      alias: 'best',
    });
  });

  it('distinct + alias together', () => {
    expect(aggregate('count', 'id', { distinct: true, alias: 'n' })).toEqual({
      type: 'aggregate',
      func: 'count',
      field: ['id'],
      distinct: true,
      alias: 'n',
    });
  });
});

describe('count', () => {
  it('default field=null targets * (field: null)', () => {
    const item = count();
    expect(item).toEqual({
      type: 'aggregate',
      func: 'count',
      field: null,
      distinct: false,
    });
    expect(item).not.toHaveProperty('alias');
  });

  it('string field normalises to path array', () => {
    expect(count('x')).toEqual({
      type: 'aggregate',
      func: 'count',
      field: ['x'],
      distinct: false,
    });
  });

  it('distinct and alias options', () => {
    expect(count('id', { distinct: true, alias: 'n' })).toEqual({
      type: 'aggregate',
      func: 'count',
      field: ['id'],
      distinct: true,
      alias: 'n',
    });
  });
});

describe('sum', () => {
  it('emits correct func discriminator and normalised field', () => {
    expect(sum('x')).toEqual({
      type: 'aggregate',
      func: 'sum',
      field: ['x'],
      distinct: false,
    });
  });
});

describe('avg', () => {
  it('emits correct func discriminator and normalised field', () => {
    expect(avg('x')).toEqual({
      type: 'aggregate',
      func: 'avg',
      field: ['x'],
      distinct: false,
    });
  });
});

describe('min', () => {
  it('emits correct func discriminator and normalised field', () => {
    expect(min('x')).toEqual({
      type: 'aggregate',
      func: 'min',
      field: ['x'],
      distinct: false,
    });
  });
});

describe('max', () => {
  it('emits correct func discriminator and normalised field', () => {
    expect(max('x')).toEqual({
      type: 'aggregate',
      func: 'max',
      field: ['x'],
      distinct: false,
    });
  });
});

describe('aggregateFn', () => {
  it('string field normalises to path array; distinct defaults false', () => {
    const item = aggregateFn('median', 'score');
    expect(item).toEqual({
      type: 'aggregate_fn',
      name: 'median',
      field: ['score'],
      distinct: false,
    });
    expect(item).not.toHaveProperty('alias');
  });

  it('array field is kept as-is', () => {
    expect(aggregateFn('stddev', ['a', 'b'])).toEqual({
      type: 'aggregate_fn',
      name: 'stddev',
      field: ['a', 'b'],
      distinct: false,
    });
  });

  it('null field stays null', () => {
    expect(aggregateFn('count_distinct', null)).toEqual({
      type: 'aggregate_fn',
      name: 'count_distinct',
      field: null,
      distinct: false,
    });
  });

  it('distinct:true overrides the default', () => {
    expect(aggregateFn('mode', 'x', { distinct: true })).toEqual({
      type: 'aggregate_fn',
      name: 'mode',
      field: ['x'],
      distinct: true,
    });
  });

  it('alias is added when provided', () => {
    expect(aggregateFn('median', 'val', { alias: 'm' })).toEqual({
      type: 'aggregate_fn',
      name: 'median',
      field: ['val'],
      distinct: false,
      alias: 'm',
    });
  });
});

describe('func', () => {
  it('name only defaults args to [] and omits alias', () => {
    const item = func('strings/upper');
    expect(item).toEqual({
      type: 'function',
      name: 'strings/upper',
      args: [],
    });
    expect(item).not.toHaveProperty('alias');
  });

  it('with args and alias', () => {
    expect(func('math/abs', [42], 'abs_val')).toEqual({
      type: 'function',
      name: 'math/abs',
      args: [42],
      alias: 'abs_val',
    });
  });

  it('args are emitted even when empty array is passed explicitly', () => {
    expect(func('strings/trim', [])).toEqual({
      type: 'function',
      name: 'strings/trim',
      args: [],
    });
  });
});

describe('select namespace', () => {
  it('exposes every constructor as a function', () => {
    expect(typeof select.all).toBe('function');
    expect(typeof select.field).toBe('function');
    expect(typeof select.countAll).toBe('function');
    expect(typeof select.aggregate).toBe('function');
    expect(typeof select.count).toBe('function');
    expect(typeof select.sum).toBe('function');
    expect(typeof select.avg).toBe('function');
    expect(typeof select.min).toBe('function');
    expect(typeof select.max).toBe('function');
    expect(typeof select.aggregateFn).toBe('function');
    expect(typeof select.func).toBe('function');
  });
});
