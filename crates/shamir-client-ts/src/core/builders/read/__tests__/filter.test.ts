/**
 * Filter constructor wire-shape tests.
 *
 * The authority for every shape is `crates/shamir-query-types/src/filter/`
 * (serde: `#[serde(tag = "op", rename_all = "snake_case")]`, field paths as
 * arrays) cross-checked with the e2e suite (tests/e2e/tests/05-filters).
 */

import { describe, it, expect } from 'vitest';
import { filter, vs } from '../filter.js';

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

describe('value-vs-value comparison (#651)', () => {
  it('valueEq / valueNe / valueGt / valueGte / valueLt / valueLte', () => {
    expect(filter.valueEq(1, 1)).toEqual({ op: 'value_compare', left: 1, cmp: 'eq', right: 1 });
    expect(filter.valueNe(1, 2)).toEqual({ op: 'value_compare', left: 1, cmp: 'ne', right: 2 });
    expect(filter.valueGt(100, 40)).toEqual({
      op: 'value_compare',
      left: 100,
      cmp: 'gt',
      right: 40,
    });
    expect(filter.valueGte(100, 40)).toEqual({
      op: 'value_compare',
      left: 100,
      cmp: 'gte',
      right: 40,
    });
    expect(filter.valueLt(10, 40)).toEqual({
      op: 'value_compare',
      left: 10,
      cmp: 'lt',
      right: 40,
    });
    expect(filter.valueLte(10, 40)).toEqual({
      op: 'value_compare',
      left: 10,
      cmp: 'lte',
      right: 40,
    });
  });

  it('valueGte with a $query ref — the canonical `when` scenario shape', () => {
    expect(filter.valueGte(filter.queryRef('balance_check', '[0].balance'), 40)).toEqual({
      op: 'value_compare',
      left: { $query: 'balance_check', path: '[0].balance' },
      cmp: 'gte',
      right: 40,
    });
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

  it('vector_similarity opts.efSearch sets ef_search', () => {
    const f = filter.vectorSimilarity('emb', [1, 0, 0.5], 10, {
      efSearch: 400,
    });
    expect(f).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
      ef_search: 400,
    });
  });

  it('vector_similarity opts.oversample sets oversample', () => {
    const f = filter.vectorSimilarity('emb', [1, 0, 0.5], 10, {
      oversample: 2.0,
    });
    expect(f).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
      oversample: 2,
    });
  });

  it('vector_similarity opts with both efSearch + oversample', () => {
    const f = filter.vectorSimilarity('emb', [1, 0, 0.5], 10, {
      efSearch: 400,
      oversample: 2.0,
    });
    expect(f).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
      ef_search: 400,
      oversample: 2,
    });
  });

  it('vector_similarity throws when ef_search exceeds MAX_EF_SEARCH (10_000)', () => {
    expect(() =>
      filter.vectorSimilarity('emb', [1, 0, 0.5], 10, { efSearch: 10_001 }),
    ).toThrow(/MAX_EF_SEARCH/);
  });

  it('vector_similarity accepts ef_search exactly at MAX_EF_SEARCH (10_000)', () => {
    const f = filter.vectorSimilarity('emb', [1, 0, 0.5], 10, {
      efSearch: 10_000,
    });
    expect(f).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
      ef_search: 10_000,
    });
  });

  it('vector_similarity throws when k is 0', () => {
    expect(() => filter.vectorSimilarity('emb', [1, 0, 0.5], 0)).toThrow(
      /k must be > 0/,
    );
  });

  it('vector_similarity throws when k is negative', () => {
    expect(() => filter.vectorSimilarity('emb', [1, 0, 0.5], -1)).toThrow(
      /k must be > 0/,
    );
  });

  it('vs() chain builder .efSearch().oversample().build()', () => {
    const f = vs('emb', [1, 0, 0.5], 10)
      .efSearch(400)
      .oversample(2.0)
      .build();
    expect(f).toEqual({
      op: 'vector_similarity',
      field: ['emb'],
      query: [1, 0, 0.5],
      k: 10,
      ef_search: 400,
      oversample: 2,
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

describe('value-ref constructors', () => {
  it('queryRef(alias, path) includes both keys', () => {
    expect(filter.queryRef('@user', '[0].id')).toEqual({
      $query: '@user',
      path: '[0].id',
    });
  });

  it('queryRef(alias) omits path key', () => {
    const v = filter.queryRef('@user');
    expect(v).toEqual({ $query: '@user' });
    expect('path' in (v as object)).toBe(false);
  });

  it('queryRef inside eq (single-value dependency)', () => {
    expect(filter.eq('user_id', filter.queryRef('@user', '[0].id'))).toEqual({
      op: 'eq',
      field: ['user_id'],
      value: { $query: '@user', path: '[0].id' },
    });
  });

  it('queryRef inside in_ (column / IN-expansion)', () => {
    expect(filter.in_('user_id', [filter.queryRef('@all_users', '[].id')])).toEqual({
      op: 'in',
      field: ['user_id'],
      values: [{ $query: '@all_users', path: '[].id' }],
    });
  });

  it('ref(string) normalises to a 1-element path', () => {
    expect(filter.ref('id')).toEqual({ $ref: ['id'] });
  });

  it('ref(string[]) keeps the path as-is', () => {
    expect(filter.ref(['addr', 'city'])).toEqual({ $ref: ['addr', 'city'] });
  });

  it('ref inside eq', () => {
    expect(filter.eq('a', filter.ref(['b', 'c']))).toEqual({
      op: 'eq',
      field: ['a'],
      value: { $ref: ['b', 'c'] },
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

describe('fn — system function call ($fn)', () => {
  it('filter.fn("NOW") produces Simple form (bare string)', () => {
    expect(filter.fn('NOW')).toEqual({ $fn: 'NOW' });
  });

  it('filter.fn("COALESCE", [null, "x"]) produces Complex form', () => {
    expect(filter.fn('COALESCE', [null, 'x'])).toEqual({
      $fn: { name: 'COALESCE', args: [null, 'x'] },
    });
  });

  it('filter.fn("UUID", []) collapses to Simple form (empty args)', () => {
    expect(filter.fn('UUID', [])).toEqual({ $fn: 'UUID' });
  });

  it('filter.fn inside eq — usage in a filter', () => {
    expect(filter.eq('created', filter.fn('NOW'))).toEqual({
      op: 'eq',
      field: ['created'],
      value: { $fn: 'NOW' },
    });
  });
});

describe('expr — expression ($expr)', () => {
  it('filter.expr("add", [10, 20])', () => {
    expect(filter.expr('add', [10, 20])).toEqual({
      $expr: { op: 'add', args: [10, 20] },
    });
  });

  it('filter.expr("concat", [...]) with nested $ref values', () => {
    expect(
      filter.expr('concat', [filter.ref('first'), ' ', filter.ref('last')]),
    ).toEqual({
      $expr: {
        op: 'concat',
        args: [{ $ref: ['first'] }, ' ', { $ref: ['last'] }],
      },
    });
  });

  it('filter.expr inside eq', () => {
    expect(filter.eq('total', filter.expr('add', [filter.ref('price'), 10]))).toEqual({
      op: 'eq',
      field: ['total'],
      value: { $expr: { op: 'add', args: [{ $ref: ['price'] }, 10] } },
    });
  });
});

describe('cond — conditional ($cond)', () => {
  it('filter.cond(eq, then, else) basic', () => {
    expect(filter.cond(filter.eq('active', true), 'yes', 'no')).toEqual({
      $cond: {
        if: { op: 'eq', field: ['active'], value: true },
        then: 'yes',
        else: 'no',
      },
    });
  });

  it('nested cond — else branch is another cond', () => {
    expect(
      filter.cond(
        filter.gte('score', 100),
        'vip',
        filter.cond(filter.gte('score', 50), 'regular', 'newbie'),
      ),
    ).toEqual({
      $cond: {
        if: { op: 'gte', field: ['score'], value: 100 },
        then: 'vip',
        else: {
          $cond: {
            if: { op: 'gte', field: ['score'], value: 50 },
            then: 'regular',
            else: 'newbie',
          },
        },
      },
    });
  });

  it('cond inside eq', () => {
    expect(
      filter.eq(
        'label',
        filter.cond(filter.eq('active', true), 'on', 'off'),
      ),
    ).toEqual({
      op: 'eq',
      field: ['label'],
      value: {
        $cond: {
          if: { op: 'eq', field: ['active'], value: true },
          then: 'on',
          else: 'off',
        },
      },
    });
  });
});

describe('switchCase — switch-case sugar over $cond', () => {
  it('switchCase with 3 branches equals hand-nested cond', () => {
    const built = filter.switchCase(
      [
        [filter.gte('score', 100), 'vip'],
        [filter.gte('score', 50), 'regular'],
      ],
      'newbie',
    );

    const hand = filter.cond(
      filter.gte('score', 100),
      'vip',
      filter.cond(filter.gte('score', 50), 'regular', 'newbie'),
    );

    expect(built).toEqual(hand);
    expect(built).toEqual({
      $cond: {
        if: { op: 'gte', field: ['score'], value: 100 },
        then: 'vip',
        else: {
          $cond: {
            if: { op: 'gte', field: ['score'], value: 50 },
            then: 'regular',
            else: 'newbie',
          },
        },
      },
    });
  });
});

describe('param — batch parameter reference ($param)', () => {
  it('filter.param returns { $param: name }', () => {
    expect(filter.param('uid')).toEqual({ $param: 'uid' });
  });

  it('param in a value position (eq)', () => {
    expect(filter.eq('user_id', filter.param('uid'))).toEqual({
      op: 'eq',
      field: ['user_id'],
      value: { $param: 'uid' },
    });
  });
});

describe('bin — binary literal (FilterValue::Binary)', () => {
  it('number[] is converted to Uint8Array', () => {
    const result = filter.bin([0xde, 0xad]);
    expect(result).toBeInstanceOf(Uint8Array);
    expect(Array.from(result)).toEqual([0xde, 0xad]);
  });

  it('Uint8Array passes through unchanged (same reference)', () => {
    const input = new Uint8Array([1, 2, 3]);
    const result = filter.bin(input);
    expect(result).toBe(input);
    expect(Array.from(result)).toEqual([1, 2, 3]);
  });

  it('bin inside eq gives correct wire form', () => {
    expect(filter.eq(['b'], filter.bin([0, 1]))).toEqual({
      op: 'eq',
      field: ['b'],
      value: new Uint8Array([0, 1]),
    });
  });
});

describe('litU64 — lossless u64 filter literal (mirrors Rust lit_u64)', () => {
  it('litU64(42) returns number 42 (within i64 range)', () => {
    expect(filter.litU64(42)).toBe(42);
  });

  it('litU64(42n) returns number 42 (within i64 range)', () => {
    const result = filter.litU64(42n);
    expect(result).toBe(42);
  });

  it('litU64(i64::MAX) stays a number at the boundary', () => {
    const max = 9223372036854775807n;
    const result = filter.litU64(max);
    expect(result).toBe(9223372036854775807);
  });

  it('litU64(i64::MAX + 1) becomes the exact decimal string', () => {
    const overflow = 9223372036854775808n; // i64::MAX + 1
    const result = filter.litU64(overflow);
    expect(result).toBe('9223372036854775808');
  });

  it('litU64(u64::MAX) becomes the exact decimal string (no sign-flip)', () => {
    const maxU64 = 18446744073709551615n;
    const result = filter.litU64(maxU64);
    expect(result).toBe('18446744073709551615');
  });

  it('litU64 inside eq gives correct wire form (small number)', () => {
    expect(filter.eq(['n'], filter.litU64(7))).toEqual({
      op: 'eq',
      field: ['n'],
      value: 7,
    });
  });

  it('litU64 inside eq gives correct wire form (overflow string)', () => {
    expect(filter.eq(['n'], filter.litU64(9223372036854775808n))).toEqual({
      op: 'eq',
      field: ['n'],
      value: '9223372036854775808',
    });
  });
});
