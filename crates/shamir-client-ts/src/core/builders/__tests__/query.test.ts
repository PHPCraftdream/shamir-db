/**
 * Query builder wire-shape tests.
 *
 * Authorities: `crates/shamir-query-types/src/read/` (serde defaults +
 * skip rules) and the e2e suite (tests/e2e/tests/07-sorting-pagination)
 * which fixes the on-the-wire `order_by` / `from` shapes.
 */

import { describe, it, expect } from 'vitest';
import { Query, atTimestamp } from '../query.js';
import { filter } from '../filter.js';
import { select } from '../select.js';

describe('from / select defaults', () => {
  it('plain SELECT * omits select (server defaults to all)', () => {
    expect(Query.from('users').build()).toEqual({ from: 'users' });
  });

  it('withRepo("main", t) collapses to a bare string', () => {
    expect(Query.withRepo('main', 'users').build()).toEqual({ from: 'users' });
  });

  it('withRepo(repo, t) emits a [repo, table] tuple', () => {
    expect(Query.withRepo('hot', 'sessions').build()).toEqual({
      from: ['hot', 'sessions'],
    });
  });

  it('select(fields) emits field items with distinct:false', () => {
    expect(Query.from('users').select(['id', 'name']).build()).toEqual({
      from: 'users',
      select: {
        items: [
          { type: 'field', path: ['id'] },
          { type: 'field', path: ['name'] },
        ],
        distinct: false,
      },
    });
  });

  it('distinct() forces select even with no explicit items', () => {
    expect(Query.from('users').distinct().build()).toEqual({
      from: 'users',
      select: { items: [{ type: 'all' }], distinct: true },
    });
  });
});

describe('where', () => {
  it('where sets the filter', () => {
    expect(Query.from('users').where(filter.eq('status', 'active')).build()).toEqual({
      from: 'users',
      where: { op: 'eq', field: ['status'], value: 'active' },
    });
  });

  it('andWhere combines with smart flattening', () => {
    const q = Query.from('users')
      .andWhere(filter.eq('status', 'active'))
      .andWhere(filter.gt('age', 18))
      .build();
    expect(q.where).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['status'], value: 'active' },
        { op: 'gt', field: ['age'], value: 18 },
      ],
    });
  });
});

// ── G1: inline where-* methods ──────────────────────────────────────

describe('inline where* methods (G1)', () => {
  it('whereEq builds an eq leaf and AND-combines', () => {
    const q = Query.from('users').whereEq('status', 'active').build();
    expect(q.where).toEqual({ op: 'eq', field: ['status'], value: 'active' });
  });

  it('whereNe / whereGt / whereGte / whereLt / whereLte', () => {
    const q = Query.from('u')
      .whereNe('a', 1)
      .whereGt('b', 2)
      .whereGte('c', 3)
      .whereLt('d', 4)
      .whereLte('e', 5)
      .build();
    expect(q.where).toEqual({
      op: 'and',
      filters: [
        { op: 'ne', field: ['a'], value: 1 },
        { op: 'gt', field: ['b'], value: 2 },
        { op: 'gte', field: ['c'], value: 3 },
        { op: 'lt', field: ['d'], value: 4 },
        { op: 'lte', field: ['e'], value: 5 },
      ],
    });
  });

  it('whereIn / whereLike', () => {
    const q = Query.from('u')
      .whereIn('id', [1, 2, 3])
      .whereLike('name', 'Al%')
      .build();
    expect(q.where).toEqual({
      op: 'and',
      filters: [
        { op: 'in', field: ['id'], values: [1, 2, 3] },
        { op: 'like', field: ['name'], pattern: 'Al%' },
      ],
    });
  });

  it('whereEq with nested path → field path array', () => {
    const q = Query.from('u').whereEq(['addr', 'city'], 'NYC').build();
    expect(q.where).toEqual({
      op: 'eq',
      field: ['addr', 'city'],
      value: 'NYC',
    });
  });

  it('orWhere* methods OR-combine', () => {
    const q = Query.from('u')
      .whereEq('a', 1)
      .orWhereEq('b', 2)
      .orWhereGt('c', 3)
      .build();
    expect(q.where).toEqual({
      op: 'or',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'eq', field: ['b'], value: 2 },
        { op: 'gt', field: ['c'], value: 3 },
      ],
    });
  });

  it('orWhereIn / orWhereLike / orWhereNe / orWhereGte / orWhereLt / orWhereLte', () => {
    const q = Query.from('u')
      .orWhereNe('a', 1)
      .orWhereGte('b', 2)
      .orWhereLt('c', 3)
      .orWhereLte('d', 4)
      .orWhereIn('e', [5])
      .orWhereLike('f', 'x%')
      .build();
    expect(q.where).toEqual({
      op: 'or',
      filters: [
        { op: 'ne', field: ['a'], value: 1 },
        { op: 'gte', field: ['b'], value: 2 },
        { op: 'lt', field: ['c'], value: 3 },
        { op: 'lte', field: ['d'], value: 4 },
        { op: 'in', field: ['e'], values: [5] },
        { op: 'like', field: ['f'], pattern: 'x%' },
      ],
    });
  });

  it('whereGroup(cb) AND-combines a nested filter', () => {
    const q = Query.from('u')
      .whereEq('status', 'active')
      .whereGroup((f) => f.or(f.eq('a', 1), f.eq('b', 2)))
      .build();
    expect(q.where).toEqual({
      op: 'and',
      filters: [
        { op: 'eq', field: ['status'], value: 'active' },
        { op: 'or', filters: [
          { op: 'eq', field: ['a'], value: 1 },
          { op: 'eq', field: ['b'], value: 2 },
        ]},
      ],
    });
  });

  it('whereGroupOr(cb) OR-combines a nested filter', () => {
    const q = Query.from('u')
      .whereEq('a', 1)
      .whereGroupOr((f) => f.and(f.eq('b', 2), f.eq('c', 3)))
      .build();
    expect(q.where).toEqual({
      op: 'or',
      filters: [
        { op: 'eq', field: ['a'], value: 1 },
        { op: 'and', filters: [
          { op: 'eq', field: ['b'], value: 2 },
          { op: 'eq', field: ['c'], value: 3 },
        ]},
      ],
    });
  });

  it('andWhere + orWhere smart-flatten together', () => {
    const q = Query.from('u')
      .whereEq('a', 1)
      .andWhere(filter.eq('b', 2))
      .orWhere(filter.eq('c', 3))
      .build();
    // (a AND b) OR c  →  { or: [ { and: [a, b] }, c ] }
    expect(q.where).toEqual({
      op: 'or',
      filters: [
        { op: 'and', filters: [
          { op: 'eq', field: ['a'], value: 1 },
          { op: 'eq', field: ['b'], value: 2 },
        ]},
        { op: 'eq', field: ['c'], value: 3 },
      ],
    });
  });
});

describe('group by / having', () => {
  it('groupBy emits field-path arrays; having nests inside group_by', () => {
    const q = Query.from('orders')
      .select([select.field('status'), select.count('id', { alias: 'n' })])
      .groupBy('status')
      .having(filter.gt('n', 5))
      .build();
    expect(q.group_by).toEqual({
      fields: [['status']],
      having: { op: 'gt', field: ['n'], value: 5 },
    });
  });

  it('having without groupBy throws on build', () => {
    expect(() => Query.from('orders').having(filter.gt('n', 5)).build()).toThrow(
      /having\(\) requires groupBy\(\)/,
    );
  });
});

describe('order by', () => {
  it('orderByAsc/Desc emit {field, direction} (direction always present)', () => {
    expect(
      Query.from('items').orderByAsc('score').orderByDesc('id').build().order_by,
    ).toEqual({
      items: [
        { field: ['score'], direction: 'asc' },
        { field: ['id'], direction: 'desc' },
      ],
    });
  });

  it('matches the e2e multi-field order_by shape', () => {
    // tests/e2e/tests/07-sorting-pagination: bucket asc, score desc
    expect(
      Query.from('items')
        .orderByAsc('bucket')
        .orderByDesc('score')
        .build().order_by,
    ).toEqual({
      items: [
        { field: ['bucket'], direction: 'asc' },
        { field: ['score'], direction: 'desc' },
      ],
    });
  });

  it('nulls ordering is carried when set', () => {
    expect(
      Query.from('items').orderByAsc('score', 'last').build().order_by,
    ).toEqual({ items: [{ field: ['score'], direction: 'asc', nulls: 'last' }] });
  });
});

describe('pagination', () => {
  it('limit+offset emits LimitOffset with offset always present', () => {
    expect(Query.from('items').limit(20).offset(40).build().pagination).toEqual(
      { mode: 'LimitOffset', offset: 40, limit: 20 },
    );
  });

  it('limit alone defaults offset to 0', () => {
    expect(Query.from('items').limit(10).build().pagination).toEqual({
      mode: 'LimitOffset',
      offset: 0,
      limit: 10,
    });
  });

  it('page emits Page mode', () => {
    expect(Query.from('items').page(2, 25).build().pagination).toEqual({
      mode: 'Page',
      page: 2,
      page_size: 25,
    });
  });

  it('countTotal sets the flag only when true', () => {
    expect(Query.from('items').countTotal().build().count_total).toBe(true);
    expect(Query.from('items').build().count_total).toBeUndefined();
  });

  it('after(key, limit) emits After with key + limit', () => {
    expect(
      Query.from('t').orderByAsc('score').after([30], 2).build().pagination,
    ).toEqual({ mode: 'After', key: [30], limit: 2 });
  });

  it('after(key) without limit omits the limit key', () => {
    const p = Query.from('t').orderByAsc('score').after([30]).build().pagination;
    expect(p).toEqual({ mode: 'After', key: [30] });
    expect(p).not.toHaveProperty('limit');
  });
});

describe('temporal', () => {
  it('default read omits temporal entirely (Latest)', () => {
    expect(Query.from('items').build().temporal).toBeUndefined();
  });

  it('asOfVersion emits {kind:"as_of", at:{version}}', () => {
    expect(Query.from('items').asOfVersion(5).build().temporal).toEqual({
      kind: 'as_of',
      at: { version: 5 },
    });
  });

  it('asOfTimestamp / asOf(at) emit {kind:"as_of", at:{timestamp}}', () => {
    expect(Query.from('items').asOfTimestamp(1700).build().temporal).toEqual({
      kind: 'as_of',
      at: { timestamp: 1700 },
    });
    expect(Query.from('items').asOf(atTimestamp(42)).build().temporal).toEqual({
      kind: 'as_of',
      at: { timestamp: 42 },
    });
  });

  it('history() defaults order to "asc" and omits empty bounds', () => {
    expect(Query.from('items').history().build().temporal).toEqual({
      kind: 'history',
      order: 'asc',
    });
  });

  it('history with bounds + desc order', () => {
    expect(
      Query.from('items')
        .history({
          from: { version: 1 },
          to: { version: 9 },
          limit: 100,
          order: 'desc',
        })
        .build().temporal,
    ).toEqual({
      kind: 'history',
      from: { version: 1 },
      to: { version: 9 },
      limit: 100,
      order: 'desc',
    });
  });

  it('withVersion sets the flag only when true', () => {
    expect(Query.from('items').withVersion().build().with_version).toBe(true);
    expect(Query.from('items').build().with_version).toBeUndefined();
  });
});

describe('aggregations', () => {
  it('count_all + sum emit aggregate items with distinct always present', () => {
    const q = Query.from('orders')
      .select([
        select.countAll('total'),
        select.sum('amount', { alias: 'revenue' }),
      ])
      .build();
    expect(q.select).toEqual({
      items: [
        { type: 'count_all', alias: 'total' },
        {
          type: 'aggregate',
          func: 'sum',
          field: ['amount'],
          distinct: false,
          alias: 'revenue',
        },
      ],
      distinct: false,
    });
  });

  it('count(null) targets * (field: null)', () => {
    expect(Query.from('orders').select([select.count()]).build().select).toEqual(
      {
        items: [{ type: 'aggregate', func: 'count', field: null, distinct: false }],
        distinct: false,
      },
    );
  });
});

describe('composed query', () => {
  it('assembles a full read query in wire order', () => {
    const q = Query.from('users')
      .select(['id', 'name'])
      .where(filter.eq('status', 'active'))
      .andWhere(filter.gt('age', 18))
      .orderByDesc('age')
      .limit(20)
      .build();
    expect(q).toEqual({
      from: 'users',
      select: {
        items: [
          { type: 'field', path: ['id'] },
          { type: 'field', path: ['name'] },
        ],
        distinct: false,
      },
      where: {
        op: 'and',
        filters: [
          { op: 'eq', field: ['status'], value: 'active' },
          { op: 'gt', field: ['age'], value: 18 },
        ],
      },
      order_by: { items: [{ field: ['age'], direction: 'desc' }] },
      pagination: { mode: 'LimitOffset', offset: 0, limit: 20 },
    });
  });
});

// ── Finding 1.3: EXPLAIN ─────────────────────────────────────────────
//
// `ReadQuery::explain` (read_query.rs, `skip_serializing_if = is_false`) was
// entirely missing from the TS type + builder — EXPLAIN was unavailable to TS
// callers. These pin the field + `.explain()` builder method + wire shape.

describe('EXPLAIN (Finding 1.3)', () => {
  it('omits explain by default (matches the false skip-serialize default)', () => {
    const q = Query.from('users').build();
    expect(q.explain).toBeUndefined();
  });

  it('.explain() emits explain: true', () => {
    const q = Query.from('users').explain().build();
    expect(q.explain).toBe(true);
  });

  it('.explain(false) omits the field again', () => {
    const q = Query.from('users').explain(true).explain(false).build();
    expect(q.explain).toBeUndefined();
  });

  it('explain combines with a normal WHERE without disturbing it', () => {
    const q = Query.from('users').whereEq('status', 'active').explain().build();
    expect(q.explain).toBe(true);
    expect(q.where).toEqual({ op: 'eq', field: ['status'], value: 'active' });
  });
});
