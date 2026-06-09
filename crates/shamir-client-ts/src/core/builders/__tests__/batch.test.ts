/**
 * Batch builder + response-type tests (vitest, NO server).
 *
 * PLATFORM-AGNOSTIC.
 */

import { describe, it, expect } from 'vitest';
import { Batch } from '../batch.js';
import { Query } from '../query.js';
import { write } from '../write.js';
import type {
  BatchResponse,
  BatchRequest,
} from '../../types/batch.js';

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

  it('limits fills missing fields with defaults', () => {
    const req = Batch.create()
      .add('u', Query.from('users'))
      .limits({ max_queries: 20 })
      .build();
    expect(req.limits).toEqual({
      max_queries: 20,
      max_dependency_depth: 10,
      max_execution_time_secs: 30,
      max_result_size: 10_485_760,
    });
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
