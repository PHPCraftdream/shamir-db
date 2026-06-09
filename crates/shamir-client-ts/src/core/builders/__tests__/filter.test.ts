/**
 * Filter constructor wire-shape tests.
 *
 * The authority for every shape is `crates/shamir-query-types/src/filter/`
 * (serde: `#[serde(tag = "op", rename_all = "snake_case")]`, field paths as
 * arrays) cross-checked with the e2e suite (tests/e2e/tests/05-filters).
 */

import { describe, it, expect } from 'vitest';
import { filter } from '../filter.js';

describe('comparison leaves', () => {
  it('eq normalises a bare field to a path array', () => {
    expect(filter.eq('age', 30)).toEqual({ op: 'eq', field: ['age'], value: 30 });
  });

  it('eq keeps an explicit nested path', () => {
    expect(filter.eq(['address', 'city'], 'NY')).toEqual({
      op: 'eq',
      field: ['address', 'city'],
      value: 'NY',
    });
  });

  it('ne / gt / gte / lt / lte', () => {
    expect(filter.ne('a', 1)).toEqual({ op: 'ne', field: ['a'], value: 1 });
    expect(filter.gt('a', 1)).toEqual({ op: 'gt', field: ['a'], value: 1 });
    expect(filter.gte('a', 1)).toEqual({ op: 'gte', field: ['a'], value: 1 });
    expect(filter.lt('a', 1)).toEqual({ op: 'lt', field: ['a'], value: 1 });
    expect(filter.lte('a', 1)).toEqual({ op: 'lte', field: ['a'], value: 1 });
  });
});

describe('field-equality shortcut', () => {
  it('serialises as op "field" (the FieldEq variant)', () => {
    expect(filter.fieldEq('status', 'active')).toEqual({
      op: 'field',
      field: ['status'],
      value: 'active',
    });
  });
});

describe('set membership', () => {
  it('in / not_in', () => {
    expect(filter.in_('id', ['a', 'b'])).toEqual({
      op: 'in',
      field: ['id'],
      values: ['a', 'b'],
    });
    expect(filter.notIn('id', [1, 2])).toEqual({
      op: 'not_in',
      field: ['id'],
      values: [1, 2],
    });
  });
});

describe('pattern matching', () => {
  it('like / ilike (op i_like) / regex', () => {
    expect(filter.like('name', 'A%')).toEqual({
      op: 'like',
      field: ['name'],
      pattern: 'A%',
    });
    expect(filter.ilike('name', 'a%')).toEqual({
      op: 'i_like',
      field: ['name'],
      pattern: 'a%',
    });
    expect(filter.regex('name', '^a')).toEqual({
      op: 'regex',
      field: ['name'],
      pattern: '^a',
    });
  });
});

describe('null / existence', () => {
  it('is_null / is_not_null / exists / not_exists', () => {
    expect(filter.isNull('x')).toEqual({ op: 'is_null', field: ['x'] });
    expect(filter.isNotNull('x')).toEqual({ op: 'is_not_null', field: ['x'] });
    expect(filter.exists('x')).toEqual({ op: 'exists', field: ['x'] });
    expect(filter.notExists('x')).toEqual({ op: 'not_exists', field: ['x'] });
  });
});

describe('containment', () => {
  it('contains / contains_any / contains_all', () => {
    expect(filter.contains('tags', 'red')).toEqual({
      op: 'contains',
      field: ['tags'],
      value: 'red',
    });
    expect(filter.containsAny('tags', ['red', 'blue'])).toEqual({
      op: 'contains_any',
      field: ['tags'],
      values: ['red', 'blue'],
    });
    expect(filter.containsAll('tags', ['red', 'blue'])).toEqual({
      op: 'contains_all',
      field: ['tags'],
      values: ['red', 'blue'],
    });
  });
});

describe('range', () => {
  it('between carries from/to', () => {
    expect(filter.between('age', 18, 65)).toEqual({
      op: 'between',
      field: ['age'],
      from: 18,
      to: 65,
    });
  });
});

describe('index-accelerated operators', () => {
  it('fts defaults to mode "and"', () => {
    expect(filter.fts('body', 'hello world')).toEqual({
      op: 'fts',
      field: ['body'],
      query: 'hello world',
      mode: 'and',
    });
  });

  it('fts honours explicit mode "or"', () => {
    expect(filter.fts('body', 'a b', 'or')).toEqual({
      op: 'fts',
      field: ['body'],
      query: 'a b',
      mode: 'or',
    });
  });

  it('vector_similarity carries query + k', () => {
    expect(filter.vectorSimilarity('emb', [1, 0, 0.5], 10)).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
    });
  });

  it('computed omits expr_args when absent', () => {
    expect(filter.computed('lower', 'email', 'eq', 'alice')).toEqual({
      op: 'computed',
      expr_op: 'lower',
      field: ['email'],
      cmp: 'eq',
      value: 'alice',
    });
  });

  it('computed includes expr_args when given', () => {
    expect(filter.computed('substring', 'name', 'eq', 'al', [0, 2])).toEqual({
      op: 'computed',
      expr_op: 'substring',
      field: ['name'],
      cmp: 'eq',
      value: 'al',
      expr_args: [0, 2],
    });
  });
});

describe('logical combinators', () => {
  it('and(a, b) wraps two leaves', () => {
    expect(filter.and(filter.eq('a', 1), filter.gt('b', 2))).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'gt', field: ['b'], value: 2 },
      ],
    });
  });

  it('and flattens when the left is already an and', () => {
    const base = filter.and(filter.eq('a', 1), filter.eq('b', 2));
    expect(filter.and(base, filter.eq('c', 3))).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'eq', field: ['b'], value: 2 },
        { op: 'eq', field: ['c'], value: 3 },
      ],
    });
  });

  it('and(array) takes an explicit list', () => {
    expect(filter.and([filter.eq('a', 1), filter.eq('b', 2)])).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'eq', field: ['b'], value: 2 },
      ],
    });
  });

  it('or flattens when the left is already an or', () => {
    const base = filter.or(filter.eq('a', 1), filter.eq('b', 2));
    expect(filter.or(base, filter.eq('c', 3))).toEqual({
      op: 'or',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'eq', field: ['b'], value: 2 },
        { op: 'eq', field: ['c'], value: 3 },
      ],
    });
  });

  it('not negates a filter', () => {
    expect(filter.not(filter.eq('a', 1))).toEqual({
      op: 'not',
      filter: { op: 'eq', field: ['a'], value: 1 },
    });
  });
});

describe('special filter values', () => {
  it('carries a $ref field reference', () => {
    expect(filter.eq('a', { $ref: ['b'] })).toEqual({
      op: 'eq',
      field: ['a'],
      value: { $ref: ['b'] },
    });
  });

  it('carries a $fn simple call', () => {
    expect(filter.eq('created', { $fn: 'NOW' })).toEqual({
      op: 'eq',
      field: ['created'],
      value: { $fn: 'NOW' },
    });
  });
});
