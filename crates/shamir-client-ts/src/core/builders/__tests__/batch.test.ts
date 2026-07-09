/**
 * Batch builder + response-type tests (vitest, NO server).
 *
 * PLATFORM-AGNOSTIC.
 */

import { describe, it, expect } from 'vitest';
import { Batch } from '../batch.js';
import { Query } from '../query.js';
import { write } from '../write.js';
import { filter } from '../filter.js';
import { Handle, RowRef } from '../handle.js';
import type {
  BatchResponse,
  BatchRequest,
  QueryResult,
} from '../../types/batch.js';
import type { InsertOp } from '../../types/write.js';
import type { ExplainPlan } from '../../types/query.js';

// ── minimal batch ───────────────────────────────────────────────────

describe('Batch — minimal build', () => {
  it('creates a minimal batch with a Query builder', () => {
    const req = Batch.create(1)
      .add('u', Query.from('users'))
      .build();

    expect(req.id).toBe(1);
    expect(req.queries).toEqual({
      u: { from: 'users' },
    });
    expect(req.transactional).toBeUndefined();
    expect(req.return_all).toBeUndefined();
    expect(req.name).toBeUndefined();
  });

  it('accepts a raw op object unchanged', () => {
    const req = Batch.create('batch-42')
      .add('ins', { insert_into: 'users', values: [{ name: 'Alice' }] })
      .build();

    expect(req.id).toBe('batch-42');
    expect(req.queries.ins).toEqual({
      insert_into: 'users',
      values: [{ name: 'Alice' }],
    });
  });

  it('defaults id to 1', () => {
    const req = Batch.create().add('q', Query.from('t')).build();
    expect(req.id).toBe(1);
  });
});

// ── return_result / after ────────────────────────────────────────────

describe('Batch — return_result and after options', () => {
  it('omits return_result by default (true)', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .build();
    expect(req.queries.u.return_result).toBeUndefined();
  });

  it('emits return_result: false when opts.returnResult === false', () => {
    const req = Batch.create()
      .add('u', Query.from('users'), { returnResult: false })
      .build();
    expect(req.queries.u.return_result).toBe(false);
  });

  it('emits after when non-empty', () => {
    const req = Batch.create()
      .add('a', Query.from('users'))
      .add('b', Query.from('orders'), { after: ['a'] })
      .build();
    expect(req.queries.b.after).toEqual(['a']);
  });

  it('omits after when empty array', () => {
    const req = Batch.create()
      .add('a', Query.from('users'), { after: [] })
      .build();
    expect(req.queries.a.after).toBeUndefined();
  });
});

// ── transactional ───────────────────────────────────────────────────

describe('Batch — transactional', () => {
  it('transactional with isolation', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .transactional('serializable')
      .build();
    expect(req.transactional).toBe(true);
    expect(req.isolation).toBe('serializable');
  });

  it('transactional without isolation', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .transactional()
      .build();
    expect(req.transactional).toBe(true);
    expect(req.isolation).toBeUndefined();
  });

  it('omits transactional when not set', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .build();
    expect(req.transactional).toBeUndefined();
  });
});

// ── durability / name / returnOnly / limits ─────────────────────────

describe('Batch — durability, name, returnOnly, limits', () => {
  it('durability("synced") emits durability field', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .durability('synced')
      .build();
    expect(req.durability).toBe('synced');
  });

  it('durability("async_index") is accepted (Finding 1.3 — was missing)', () => {
    // `'async_index'` is a first-class DurabilityLevel in Rust
    // (batch_request.rs); it must type-check AND round-trip through the builder.
    const req = Batch.create()
      .add('u', Query.from('users'))
      .durability('async_index')
      .build();
    expect(req.durability).toBe('async_index');
  });

  it('name emits name field', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .name('b')
      .build();
    expect(req.name).toBe('b');
  });

  it('returnOnly emits return_only array', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .add('o', Query.from('orders'))
      .returnOnly(['u'])
      .build();
    expect(req.return_only).toEqual(['u']);
  });

  it('limits fills missing fields with defaults (full 5-field Rust BatchLimits parity)', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .limits({ max_queries: 20 })
      .build();
    // Exact 5-field shape mirroring Rust `BatchLimits::default()` — a
    // missing `max_nesting_depth` here would have caused the old bug to
    // pass silently (server rejects the whole batch with
    // `invalid_request: missing field 'max_nesting_depth'`).
    expect(req.limits).toEqual({
      max_queries: 20,
      max_dependency_depth: 10,
      max_execution_time_secs: 30,
      max_result_size: 10_485_760,
      max_nesting_depth: 4,
    });
    // Precise key-set check: catches a future 6th field drift too.
    expect(Object.keys(req.limits!).sort()).toEqual([
      'max_dependency_depth',
      'max_execution_time_secs',
      'max_nesting_depth',
      'max_queries',
      'max_result_size',
    ]);
  });

  it('limits round-trips through JSON with all 5 fields present', () => {
    // This is the regression test that would have caught CRIT-8 pre-fix:
    // the server deserialises `BatchLimits` with all-fields-required
    // serde semantics, so every field must survive a JSON encode/decode.
    const req = Batch.create()
      .add('u', Query.from('users'))
      .limits({ max_queries: 20 })
      .build();
    const decoded = JSON.parse(JSON.stringify(req));
    expect(decoded.limits).toEqual({
      max_queries: 20,
      max_dependency_depth: 10,
      max_execution_time_secs: 30,
      max_result_size: 10_485_760,
      max_nesting_depth: 4,
    });
    expect(Object.keys(decoded.limits)).toHaveLength(5);
  });

  it('limits honours an explicit max_nesting_depth override', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .limits({ max_nesting_depth: 0 })
      .build();
    expect(req.limits?.max_nesting_depth).toBe(0);
  });

  it('omits fields when not set', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .build();
    expect(req.durability).toBeUndefined();
    expect(req.name).toBeUndefined();
    expect(req.return_only).toBeUndefined();
    expect(req.limits).toBeUndefined();
  });
});

// ── returnAll ───────────────────────────────────────────────────────

describe('Batch — returnAll', () => {
  it('omits return_all when true (default)', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .build();
    expect(req.return_all).toBeUndefined();
  });

  it('emits return_all: false when set to false', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .returnAll(false)
      .build();
    expect(req.return_all).toBe(false);
  });
});

// ── insert helper interop ───────────────────────────────────────────

describe('Batch — insert helper interop', () => {
  it('accepts an InsertOp from the write helper', () => {
    const req = Batch.create()
      .add('ins', write.insert('users', [{ name: 'Alice' }]))
      .build();
    expect(req.queries.ins).toEqual({
      insert_into: 'users',
      values: [{ name: 'Alice' }],
    });
  });
});

// ── subBatch ─────────────────────────────────────────────────────────

describe('Batch — subBatch', () => {
  it('produces { batch, bind } wire shape', () => {
    const inner = Batch.create('inner')
      .add('item', Query.from('items'))
      .build();

    const req = Batch.create('f')
      .add('user', Query.from('users').where(filter.eq('id', 'u1')))
      .subBatch('proc', inner, {
        bind: { uid: filter.queryRef('@user', '[0].id') },
      })
      .build();

    expect(req.queries.proc).toEqual({
      batch: inner,
      bind: { uid: { $query: '@user', path: '[0].id' } },
    });
  });

  it('subBatch omits empty bind', () => {
    const inner = Batch.create('inner')
      .add('item', Query.from('items'))
      .build();

    const req = Batch.create('f')
      .subBatch('proc', inner, { bind: {} })
      .build();

    const entry = req.queries.proc as { batch: BatchRequest; bind?: unknown };
    expect(entry.batch).toEqual(inner);
    expect(entry.bind).toBeUndefined();
  });

  it('subBatch accepts a raw BatchRequest (not a Batch instance)', () => {
    const rawBatch: BatchRequest = {
      id: 'raw',
      queries: {
        q: { from: 'items' },
      },
    };

    const req = Batch.create('outer')
      .subBatch('nested', rawBatch, {
        bind: { x: filter.param('uid') },
      })
      .build();

    expect(req.queries.nested).toEqual({
      batch: rawBatch,
      bind: { x: { $param: 'uid' } },
    });
  });

  it('subBatch accepts a Batch instance and calls .build()', () => {
    const innerBuilder = Batch.create('b')
      .add('q', Query.from('orders'));

    const req = Batch.create('outer')
      .subBatch('child', innerBuilder)
      .build();

    const entry = req.queries.child as { batch: BatchRequest };
    expect(entry.batch).toEqual(innerBuilder.build());
  });

  it('subBatch respects returnResult and after opts', () => {
    const inner = Batch.create('i').add('q', Query.from('t')).build();

    const req = Batch.create('o')
      .subBatch('x', inner, { returnResult: false, after: ['a'] })
      .build();

    expect(req.queries.x.return_result).toBe(false);
    expect(req.queries.x.after).toEqual(['a']);
  });
});

// ── response type smoke test ────────────────────────────────────────

describe('Batch — response type smoke', () => {
  it('BatchResponse shape is accessible', () => {
    const resp: BatchResponse = {
      id: 1,
      results: {
        u: {
          records: [{ id: 1, name: 'Alice' }],
          stats: {
            index_used: null,
            records_scanned: 1,
            records_returned: 1,
            execution_time_us: 42,
          },
        },
      },
      execution_plan: [['u']],
      execution_time_us: 100,
      transaction: {
        tx_id: 7,
        status: 'committed',
        snapshot_version: 5,
        commit_version: 6,
        materialized: true,
      },
    };

    expect(resp.results.u.records).toEqual([{ id: 1, name: 'Alice' }]);
    expect(resp.execution_plan).toEqual([['u']]);
    expect(resp.transaction?.status).toBe('committed');
  });

  it('BatchResponse without transaction', () => {
    const resp: BatchResponse = {
      id: 'b-99',
      results: {},
      execution_plan: [],
      execution_time_us: 0,
    };
    expect(resp.transaction).toBeUndefined();
    expect(resp.results).toEqual({});
  });
});

// ── G3: typed Handle / RowRef ───────────────────────────────────────

describe('Batch — Handle / RowRef (G3)', () => {
  it('handle() returns a Handle for a registered alias', () => {
    const b = Batch.create().add('u', Query.from('users'));
    const h = b.handle('u');
    expect(h).toBeInstanceOf(Handle);
  });

  it('ref() is an alias for handle()', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.ref('u')).toBeInstanceOf(Handle);
  });

  it('handle() throws for an unregistered alias', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(() => b.handle('nope')).toThrow(/not registered/);
  });

  it('handle() does not break add() chaining', () => {
    // add() still returns this; handle() is a separate accessor.
    const b = Batch.create()
      .add('u', Query.from('users'))
      .add('o', Query.from('orders'));
    expect(b.handle('u').column('id')).toEqual({ $query: 'u', path: '[].id' });
  });

  it('Handle.column(field) → $query path "[].field"', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').column('id')).toEqual({ $query: 'u', path: '[].id' });
  });

  it('Handle.column(nested) → "[].a.b"', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').column(['addr', 'city'])).toEqual({
      $query: 'u',
      path: '[].addr.city',
    });
  });

  it('Handle.row(i) returns a RowRef', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').row(2)).toBeInstanceOf(RowRef);
  });

  it('Handle.first() = row(0)', () => {
    const b = Batch.create().add('u', Query.from('users'));
    const first = b.handle('u').first();
    expect(first.field('id')).toEqual({ $query: 'u', path: '[0].id' });
  });

  it('Handle.all() → bare $query ref (no path)', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').all()).toEqual({ $query: 'u' });
  });

  it('RowRef.field(f) → "[i].field"', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').row(3).field('name')).toEqual({
      $query: 'u',
      path: '[3].name',
    });
  });

  it('RowRef.field(nested) → "[i].a.b"', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').row(1).field(['profile', 'age'])).toEqual({
      $query: 'u',
      path: '[1].profile.age',
    });
  });

  it('RowRef.get() → "[i]"', () => {
    const b = Batch.create().add('u', Query.from('users'));
    expect(b.handle('u').row(5).get()).toEqual({ $query: 'u', path: '[5]' });
  });

  it('Handle.column() wires into a downstream query filter', () => {
    // Real usage: build the batch in two steps so the Handle is materialised
    // before the downstream query references it.
    const step1 = Batch.create().add('u', Query.from('users'));
    const userIds = step1.handle('u').column('id');
    const b = step1
      .add('o', Query.from('orders').where(filter.eq('user_id', userIds)))
      .build();
    expect((b.queries.o as { where: unknown }).where).toEqual({
      op: 'eq',
      field: ['user_id'],
      value: { $query: 'u', path: '[].id' },
    });
  });
});

// ── G4: tryBuild validation ─────────────────────────────────────────

describe('Batch — tryBuild (G4)', () => {
  it('returns the built request on success (same shape as build)', () => {
    const b = Batch.create()
      .add('u', Query.from('users'))
      .add(
        'o',
        Query.from('orders').where(filter.eq('user_id', filter.queryRef('u', '[].id'))),
        { after: ['u'] },
      );
    const req = b.tryBuild();
    expect((req.queries.o as { where: unknown }).where).toEqual({
      op: 'eq',
      field: ['user_id'],
      value: { $query: 'u', path: '[].id' },
    });
    expect(req.queries.o.after).toEqual(['u']);
  });

  it('throws when a $query ref points to an undeclared alias', () => {
    const b = Batch.create().add(
      'o',
      Query.from('orders').where(filter.eq('user_id', filter.queryRef('ghost', '[].id'))),
    );
    expect(() => b.tryBuild()).toThrow(/unknown \$query alias 'ghost'/);
  });

  it('throws when an after-dep names an undeclared alias', () => {
    const b = Batch.create().add('o', Query.from('orders'), { after: ['nope'] });
    expect(() => b.tryBuild()).toThrow(/after-dependency 'nope'/);
  });

  it('validates $query refs nested inside and/or groups', () => {
    const b = Batch.create().add(
      'o',
      Query.from('orders').where(
        filter.and(
          filter.eq('status', 'open'),
          filter.eq('uid', filter.queryRef('missing')),
        ),
      ),
    );
    expect(() => b.tryBuild()).toThrow(/unknown \$query alias 'missing'/);
  });

  it('validates $query refs inside SubBatchOp.bind', () => {
    const inner = Batch.create('inner').add('q', Query.from('items')).build();
    const b = Batch.create().subBatch('proc', inner, {
      bind: { uid: filter.queryRef('undeclared', '[0].id') },
    });
    expect(() => b.tryBuild()).toThrow(/unknown \$query alias 'undeclared'/);
  });

  it('build() stays unchecked (does not throw on bad refs)', () => {
    const b = Batch.create().add(
      'o',
      Query.from('orders').where(filter.eq('uid', filter.queryRef('ghost'))),
    );
    expect(() => b.build()).not.toThrow();
  });

  it('succeeds with no refs and no after deps', () => {
    const b = Batch.create()
      .add('a', Query.from('users'))
      .add('b', Query.from('orders'));
    expect(b.tryBuild().queries).toEqual({
      a: { from: 'users' },
      b: { from: 'orders' },
    });
  });
});

// ── Finding 1.3: wire-type drift ─────────────────────────────────────

describe('Finding 1.3 — wire-type drift fixes', () => {
  it('records_idmsgpack is typed on InsertOp (per-op), not on BatchRequest', () => {
    // Correct home: InsertOp (Rust write/types.rs::InsertOp.records_idmsgpack).
    const op: InsertOp = {
      insert_into: 'users',
      values: [],
      records_idmsgpack: [new Uint8Array([1, 2, 3])],
    };
    expect(op.records_idmsgpack?.[0]).toBeInstanceOf(Uint8Array);

    // And it is NO LONGER a batch-level field: a BatchRequest carrying it must
    // fail to type-check. `@ts-expect-error` turns a regression (re-adding the
    // batch-level field) into a compile error.
    const req: BatchRequest = {
      id: 1,
      queries: {},
      // @ts-expect-error records_idmsgpack is per-op, not batch-level
      records_idmsgpack: [new Uint8Array([9])],
    };
    expect(req.id).toBe(1);
  });

  it('QueryResult carries an optional explain plan (Finding 1.3)', () => {
    const plan: ExplainPlan = {
      plan_type: 'IndexScan',
      index_used: 'users_by_status',
      estimated_rows: 42,
    };
    const result: QueryResult = { records: [], explain: plan };
    expect(result.explain?.plan_type).toBe('IndexScan');
    expect(result.explain?.index_used).toBe('users_by_status');

    // A result without explain is still valid (skip-if-none).
    const bare: QueryResult = { records: [] };
    expect(bare.explain).toBeUndefined();
  });
});
