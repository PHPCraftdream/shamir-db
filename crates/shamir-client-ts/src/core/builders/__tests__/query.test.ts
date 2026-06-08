/**
 * Query builder wire-shape tests.
 *
 * Authorities: `crates/shamir-query-types/src/read/` (serde defaults +
 * skip rules) and the e2e suite (tests/e2e/tests/07-sorting-pagination)
 * which fixes the on-the-wire `order_by` / `from` shapes.
 */

import { describe, it, expect } from 'vitest';
import { Query, atTimestamp } from '../query.js';
import * as f from '../filter.js';
import * as select from '../select.js';

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
    expect(Query.from('users').where(f.eq('status', 'active')).build()).toEqual({
      from: 'users',
      where: { op: 'eq', field: ['status'], value: 'active' },
    });
  });

  it('andWhere combines with smart flattening', () => {
    const q = Query.from('users')
      .andWhere(f.eq('status', 'active'))
      .andWhere(f.gt('age', 18))
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

describe('group by / having', () => {
  it('groupBy emits field-path arrays; having nests inside group_by', () => {
    const q = Query.from('orders')
      .select([select.field('status'), select.count('id', { alias: 'n' })])
      .groupBy('status')
      .having(f.gt('n', 5))
      .build();
    expect(q.group_by).toEqual({
      fields: [['status']],
      having: { op: 'gt', field: ['n'], value: 5 },
    });
  });

  it('having without groupBy throws on build', () => {
    expect(() => Query.from('orders').having(f.gt('n', 5)).build()).toThrow(
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
      .where(f.eq('status', 'active'))
      .andWhere(f.gt('age', 18))
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
