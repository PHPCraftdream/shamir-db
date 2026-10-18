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
  expr,
  selectExpr,
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
      args: [],
      distinct: false,
    });
    expect(item).not.toHaveProperty('alias');
  });

  it('array field is kept as-is', () => {
    expect(aggregateFn('stddev', ['a', 'b'])).toEqual({
      type: 'aggregate_fn',
      name: 'stddev',
      field: ['a', 'b'],
      args: [],
      distinct: false,
    });
  });

  it('null field stays null', () => {
    expect(aggregateFn('count_distinct', null)).toEqual({
      type: 'aggregate_fn',
      name: 'count_distinct',
      field: null,
      args: [],
      distinct: false,
    });
  });

  it('distinct:true overrides the default', () => {
    expect(aggregateFn('mode', 'x', { distinct: true })).toEqual({
      type: 'aggregate_fn',
      name: 'mode',
      field: ['x'],
      args: [],
      distinct: true,
    });
  });

  it('alias is added when provided', () => {
    expect(aggregateFn('median', 'val', { alias: 'm' })).toEqual({
      type: 'aggregate_fn',
      name: 'median',
      field: ['val'],
      args: [],
      distinct: false,
      alias: 'm',
    });
  });

  it('args are passed through for parameterised aggregates', () => {
    expect(
      aggregateFn('percentile', 'score', { alias: 'p90', args: [0.9] }),
    ).toEqual({
      type: 'aggregate_fn',
      name: 'percentile',
      field: ['score'],
      args: [0.9],
      distinct: false,
      alias: 'p90',
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

describe('expr', () => {
  it('no alias omits the alias key (#1024)', () => {
    const item = expr(selectExpr.literal(1));
    expect(item).toEqual({ type: 'expr', expr: { op: 'literal', value: 1 } });
    expect(item).not.toHaveProperty('alias');
  });

  it('with alias adds the alias key', () => {
    const item = expr(
      selectExpr.add(selectExpr.field('price'), selectExpr.literal(1)),
      'bumped',
    );
    expect(item).toEqual({
      type: 'expr',
      expr: {
        op: 'add',
        left: { op: 'field', path: ['price'] },
        right: { op: 'literal', value: 1 },
      },
      alias: 'bumped',
    });
  });
});

describe('selectExpr', () => {
  it('add/sub/mul/div build the matching binary op shape', () => {
    const a = selectExpr.literal(1);
    const b = selectExpr.literal(2);
    expect(selectExpr.add(a, b)).toEqual({ op: 'add', left: a, right: b });
    expect(selectExpr.sub(a, b)).toEqual({ op: 'sub', left: a, right: b });
    expect(selectExpr.mul(a, b)).toEqual({ op: 'mul', left: a, right: b });
    expect(selectExpr.div(a, b)).toEqual({ op: 'div', left: a, right: b });
  });

  it('field normalises a bare string spec to a path array', () => {
    expect(selectExpr.field('age')).toEqual({ op: 'field', path: ['age'] });
  });

  it('field keeps an array spec as-is', () => {
    expect(selectExpr.field(['address', 'zip'])).toEqual({
      op: 'field',
      path: ['address', 'zip'],
    });
  });

  it('literal wraps null/bool/number/string values', () => {
    expect(selectExpr.literal(null)).toEqual({ op: 'literal', value: null });
    expect(selectExpr.literal(true)).toEqual({ op: 'literal', value: true });
    expect(selectExpr.literal(42)).toEqual({ op: 'literal', value: 42 });
    expect(selectExpr.literal('x')).toEqual({ op: 'literal', value: 'x' });
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
    expect(typeof select.expr).toBe('function');
  });
});
