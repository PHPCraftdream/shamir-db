/**
 * Unit tests for the bound `Db` handle — Layer 2 convenience over pure
 * builders. Uses a fake client to verify wire shapes and control flow
 * without a live server.
 */

import { describe, it, expect } from 'vitest';

import { Db } from '../db.js';
import type { ExecCtx } from '../exec-ctx.js';
import type { BatchResponse, QueryResult } from '../types/batch.js';
import type { WireValue } from '../types/write.js';
import { Query } from '../builders/query.js';
import { Batch } from '../builders/batch.js';
import { filter } from '../builders/filter.js';
import { write } from '../builders/write.js';
import * as ddl from '../builders/ddl.js';
import { SubscriptionRouter } from '../subscription-router.js';
import { subscribe } from '../builders/subscribe.js';

// ─── fake infrastructure ────────────────────────────────────────────────────

/** A captured (dbName, batch) pair. */
interface Captured {
  db: string;
  batch: object;
}

/**
 * Build a fake client that records every `execute(db, batch)` call.
 * `hmacTagHex` returns a deterministic stub so HMAC wrappers are verifiable.
 * Includes no-op tx stubs so the cast to ShamirClient satisfies the type.
 */
function fakeClient(captured: Captured[]) {
  const okResult: QueryResult = {
    records: [{ id: 'fake', ok: true }],
  };
  const okBatch: BatchResponse = {
    id: 1 as WireValue,
    results: { _: okResult },
    execution_plan: [],
    execution_time_us: 0,
  };
  return {
    execute: async (db: string, batch: object): Promise<BatchResponse> => {
      captured.push({ db, batch });
      return {
        id: ((batch as { id?: unknown }).id ?? 1) as WireValue,
        results: { _: okResult },
        execution_plan: [],
        execution_time_us: 0,
      };
    },
    txBegin: async (): Promise<import('../client.js').TxOpened> => ({
      tx_handle: 0,
      snapshot_version: 0,
      isolation: 'snapshot',
    }),
    txExecute: async (): Promise<BatchResponse> => okBatch,
    txCommit: async (): Promise<import('../types/batch.js').TransactionInfo> => ({
      tx_id: 0,
      status: 'committed',
      materialized: true,
    }),
    txRollback: async (): Promise<void> => {},
    hmacTagHex: (_canonical: Uint8Array): string => {
      return 'aa'.repeat(32);
    },
  };
}

/** Cast a fake client to the minimal ShamirClient-like shape Db expects. */
function asClient(fc: ReturnType<typeof fakeClient>) {
  return fc as unknown as import('../client.js').ShamirClient;
}

// ─── tests ──────────────────────────────────────────────────────────────────

describe('Db handle (unit)', () => {
  it('db.run(write.insert(...)) sends single-op batch and returns unwrapped result', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'test_db');

    const result = await db.run(write.insert('items', [{ a: 1 }]));

    expect(captured.length).toBe(1);
    expect(captured[0].db).toBe('test_db');
    expect(captured[0].batch).toEqual({
      id: 1,
      queries: {
        _: { insert_into: 'items', values: [{ a: 1 }] },
      },
    });
    expect(result.records).toEqual([{ id: 'fake', ok: true }]);
  });

  it('db.rows(op) returns .records directly', async () => {
    const fc = fakeClient([]);
    const db = new Db(asClient(fc), 'my_app');

    const records = await db.rows(write.insert('t', [{ x: 1 }]));
    expect(records).toEqual([{ id: 'fake', ok: true }]);
  });

  it('db.query("t").where(filter.eq(...)).ex() posts single-op batch', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    const result = await db.query('items').where(filter.eq('id', 'x')).ex();

    expect(captured.length).toBe(1);
    expect(captured[0].db).toBe('my_app');
    const batch = captured[0].batch as {
      id: unknown;
      queries: Record<string, object>;
    };
    expect(batch.id).toBe(1);
    expect(batch.queries['_']).toEqual({
      from: 'items',
      where: { op: 'eq', field: ['id'], value: 'x' },
    });
    expect(result.records).toEqual([{ id: 'fake', ok: true }]);
  });

  it('db.query("t").where(...).rows() returns records', async () => {
    const fc = fakeClient([]);
    const db = new Db(asClient(fc), 'my_app');

    const records = await db.query('items').where(filter.eq('id', 'x')).rows();
    expect(records).toEqual([{ id: 'fake', ok: true }]);
  });

  it('db.batch().add(...).run() posts batch with bound query built', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    const resp = await db
      .batch('batch-1')
      .add('a', db.query('x'))
      .run();

    expect(captured.length).toBe(1);
    expect(captured[0].db).toBe('my_app');
    const batch = captured[0].batch as {
      id: unknown;
      queries: Record<string, object>;
    };
    expect(batch.id).toBe('batch-1');
    expect(batch.queries['a']).toEqual({ from: 'x' });
    expect(resp.results['_']).toEqual({
      records: [{ id: 'fake', ok: true }],
    });
  });

  it('unbound Query.ex() throws "not bound" error', async () => {
    const q = Query.from('t');
    await expect(q.ex()).rejects.toThrow(
      'Query is not bound to a Db; use db.query(...) or db.run(query)',
    );
  });

  it('unbound Batch.run() throws "not bound" error', async () => {
    const b = Batch.create();
    await expect(b.run()).rejects.toThrow(
      'Batch is not bound to a Db; use db.batch() or batch.execute(client, db)',
    );
  });

  it('db.dropTable(...) sends HMAC-signed op', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    const result = await db.dropTable('main', 'old_table');

    expect(captured.length).toBe(1);
    expect(captured[0].db).toBe('my_app');
    const batch = captured[0].batch as {
      id: unknown;
      queries: Record<string, object>;
    };
    const op = batch.queries['_'] as Record<string, unknown>;
    expect(op.drop_table).toBe('old_table');
    expect(op.repo).toBe('main');
    expect(typeof op.hmac).toBe('string');
    expect((op.hmac as string).length).toBe(64); // 32 bytes hex
    expect(result.records).toEqual([{ id: 'fake', ok: true }]);
  });

  it('db.dropIndex(...) sends HMAC-signed op with optional unique', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.dropIndex('main', 't', 'by_email', { unique: true });

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    const op = batch.queries['_'] as Record<string, unknown>;
    expect(op.drop_index).toBe('by_email');
    expect(op.unique).toBe(true);
    expect(typeof op.hmac).toBe('string');
  });

  it('db.dropRepo(...) sends HMAC-signed op', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.dropRepo('archive', { cascade: true });

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    const op = batch.queries['_'] as Record<string, unknown>;
    expect(op.drop_repo).toBe('archive');
    expect(op.cascade).toBe(true);
    expect(typeof op.hmac).toBe('string');
  });

  it('db.dropDb(...) sends HMAC-signed op', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.dropDb({ cascade: true });

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    const op = batch.queries['_'] as Record<string, unknown>;
    expect(op.drop_db).toBe('my_app');
    expect(op.cascade).toBe(true);
    expect(typeof op.hmac).toBe('string');
  });

  it('db.run() accepts a builder with .build()', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.run(write.update('items').where(filter.eq('id', 'B2')).set({ qty: 9 }));

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    const op = batch.queries['_'] as Record<string, unknown>;
    expect(op.update).toBe('items');
    expect(op.where).toEqual({ op: 'eq', field: ['id'], value: 'B2' });
    expect(op.set).toEqual({ qty: 9 });
  });

  it('db.run() accepts a raw wire op object', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.run({ create_table: 'new_table', repo: 'main' });

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    expect(batch.queries['_']).toEqual({ create_table: 'new_table', repo: 'main' });
  });

  it('Layer-1: Batch.execute(client, db) still works', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);

    const resp = await Batch.create('l1-test')
      .add('q', Query.from('t'))
      .execute({ execute: fc.execute } as unknown as import('../client.js').ShamirClient, 'l1_db');

    expect(captured.length).toBe(1);
    expect(captured[0].db).toBe('l1_db');
    expect(resp.results['_']).toEqual({
      records: [{ id: 'fake', ok: true }],
    });
  });

  it('Layer-1: unbound Query.build() still produces wire shape', () => {
    const q = Query.from('t').where(filter.eq('id', 'A1'));
    const wire = q.build();
    expect(wire).toEqual({
      from: 't',
      where: { op: 'eq', field: ['id'], value: 'A1' },
    });
  });

  it('db.batch().transactional().run() preserves transactional flag', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db
      .batch('tx-test')
      .add('ins', write.insert('t', [{ id: 'x' }]))
      .transactional()
      .run();

    const batch = captured[0].batch as {
      id: unknown;
      queries: Record<string, object>;
      transactional?: boolean;
    };
    expect(batch.transactional).toBe(true);
    expect(batch.queries['ins']).toEqual({
      insert_into: 't',
      values: [{ id: 'x' }],
    });
  });

  it('db.withRepo(repo, table) creates bound query with repo', async () => {
    const captured: Captured[] = [];
    const fc = fakeClient(captured);
    const db = new Db(asClient(fc), 'my_app');

    await db.withRepo('archive', 'orders').rows();

    const batch = captured[0].batch as {
      queries: Record<string, object>;
    };
    expect(batch.queries['_']).toEqual({ from: ['archive', 'orders'] });
  });

  // ─── runLive (A4b) ────────────────────────────────────────────────────────

  /**
   * Build a fake client wired with a real {@link SubscriptionRouter} and a
   * stubbed `execute` that returns a BatchResponse whose `messages` alias
   * carries `value = { subscription_grant: true, sources_count: 1, sub: subId }`.
   */
  function fakeLiveClient(subId: number) {
    const router = new SubscriptionRouter();
    const resp: BatchResponse = {
      id: 1 as WireValue,
      results: {
        messages: {
          records: [],
          value: { subscription_grant: true, sources_count: 1, sub: subId } as WireValue,
        },
      },
      execution_plan: [],
      execution_time_us: 0,
    };
    return {
      router,
      client: {
        execute: async (_db: string, _batch: object): Promise<BatchResponse> => resp,
        get subscriptions(): SubscriptionRouter {
          return router;
        },
      },
    };
  }

  it('db.runLive: extracts SubscriptionHandle keyed by alias from grant `sub`', async () => {
    const { router, client } = fakeLiveClient(7);
    const db = new Db(client as unknown as import('../client.js').ShamirClient, 'live_db');

    const { response, subs } = await db.runLive(
      db.batch('live-1').subscribe('messages', { store: 'main', table: 'messages' }),
    );

    expect(Object.keys(subs)).toEqual(['messages']);
    expect(subs.messages.subId).toBe(7);
    // Response is still the underlying BatchResponse (passthrough).
    expect(response.results.messages.value).toMatchObject({ sub: 7 });

    // Push routing: manually route through the client's router; the handle
    // must yield the event via its async iterator.
    const next = subs.messages.next();
    router.route({ push: 'event', sub: 7, seq: 1, data: new Uint8Array([1, 2, 3]) });
    const ev = await next;
    expect(ev.done).toBe(false);
    expect(ev.value.kind).toBe('event');
    expect(ev.value.seq).toBe(1);
    expect(ev.value.data).toEqual(new Uint8Array([1, 2, 3]));
  });

  it('db.runLive: aliases without a numeric sub are not turned into handles', async () => {
    const router = new SubscriptionRouter();
    const resp: BatchResponse = {
      id: 1 as WireValue,
      results: {
        // A regular read result — no value.sub. Must NOT produce a handle.
        plain: { records: [{ id: 'x' }] },
        // value present but not an object — must NOT produce a handle.
        scalar: { records: [], value: 42 as WireValue },
      },
      execution_plan: [],
      execution_time_us: 0,
    };
    const client = {
      execute: async (): Promise<BatchResponse> => resp,
      get subscriptions(): SubscriptionRouter {
        return router;
      },
    };
    const db = new Db(client as unknown as import('../client.js').ShamirClient, 'live_db');
    const { subs } = await db.runLive(
      db.batch('no-grant').add('plain', Query.from('t')),
    );
    expect(Object.keys(subs)).toEqual([]);
    // Use the imported `subscribe` builder symbol so the lint sees it referenced.
    expect(typeof subscribe).toBe('function');
  });
});

// ─── Tx (interactive transaction) unit tests ─────────────────────────────────

describe('Db.tx() (unit)', () => {
  /** Call log entry recorded by the fake tx-aware client. */
  interface TxCall {
    method: string;
    args: unknown[];
  }

  const okResult: QueryResult = { records: [{ ok: true }] };
  const okBatchResponse: BatchResponse = {
    id: 1 as WireValue,
    results: { _: okResult },
    execution_plan: [],
    execution_time_us: 0,
  };

  /**
   * Build a fake client that records txBegin/txExecute/txCommit/txRollback
   * and execute calls. Defaults to happy-path responses.
   */
  function fakeTxClient(
    calls: TxCall[],
    overrides?: {
      commitResponse?: import('../types/batch.js').TransactionInfo;
    },
  ) {
    const commitResp = overrides?.commitResponse ?? {
      tx_id: 1,
      status: 'committed' as const,
      materialized: true,
      commit_version: 42,
    };
    return {
      execute: async (db: string, batch: object): Promise<BatchResponse> => {
        calls.push({ method: 'execute', args: [db, batch] });
        return okBatchResponse;
      },
      txBegin: async (
        db: string,
        repo: string,
        isolation?: string,
      ): Promise<import('../client.js').TxOpened> => {
        calls.push({ method: 'txBegin', args: [db, repo, isolation] });
        return { tx_handle: 99, snapshot_version: 10, isolation: isolation ?? 'snapshot' };
      },
      txExecute: async (
        db: string,
        handle: number,
        batch: object,
      ): Promise<BatchResponse> => {
        calls.push({ method: 'txExecute', args: [db, handle, batch] });
        return okBatchResponse;
      },
      txCommit: async (
        db: string,
        handle: number,
      ): Promise<import('../types/batch.js').TransactionInfo> => {
        calls.push({ method: 'txCommit', args: [db, handle] });
        return commitResp;
      },
      txRollback: async (db: string, handle: number): Promise<void> => {
        calls.push({ method: 'txRollback', args: [db, handle] });
      },
      hmacTagHex: (_canonical: Uint8Array): string => 'aa'.repeat(32),
    };
  }

  function asClient(fc: ReturnType<typeof fakeTxClient>) {
    return fc as unknown as import('../client.js').ShamirClient;
  }

  it('happy path: begin → execute → commit', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await db.tx(async (t) => {
      await t.run(write.insert('x', [{ a: 1 }]));
    });

    expect(calls.length).toBe(3);
    expect(calls[0]).toEqual({ method: 'txBegin', args: ['test_db', 'main', undefined] });
    expect(calls[1].method).toBe('txExecute');
    expect(calls[1].args).toEqual([
      'test_db',
      99,
      { id: 1, queries: { _: { insert_into: 'x', values: [{ a: 1 }] } } },
    ]);
    expect(calls[2]).toEqual({ method: 'txCommit', args: ['test_db', 99] });
  });

  it('happy path: no txRollback on success', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await db.tx(async () => {});

    const methods = calls.map((c) => c.method);
    expect(methods).not.toContain('txRollback');
  });

  it('throw path: begin → rollback, no commit; error rethrown', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await expect(
      db.tx(async () => {
        throw new Error('boom');
      }),
    ).rejects.toThrow('boom');

    const methods = calls.map((c) => c.method);
    expect(methods).toEqual(['txBegin', 'txRollback']);
    expect(methods).not.toContain('txCommit');
  });

  it('opts.isolation is forwarded to txBegin', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await db.tx(async () => {}, { isolation: 'serializable' });

    expect(calls[0]).toEqual({
      method: 'txBegin',
      args: ['test_db', 'main', 'serializable'],
    });
  });

  it('opts.repo is forwarded to txBegin', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await db.tx(async () => {}, { repo: 'archive' });

    expect(calls[0]).toEqual({
      method: 'txBegin',
      args: ['test_db', 'archive', undefined],
    });
  });

  it('aborted commit rejects with reason', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls, {
      commitResponse: {
        tx_id: 1,
        status: 'aborted',
        reason: 'tx_conflict',
        materialized: false,
      },
    });
    const db = new Db(asClient(fc), 'test_db');

    await expect(
      db.tx(async () => {}),
    ).rejects.toThrow('transaction aborted: tx_conflict');

    // The aborted commit already finalised the tx server-side — NO redundant
    // rollback is issued; only begin + commit were attempted.
    const methods = calls.map((c) => c.method);
    expect(methods).toEqual(['txBegin', 'txCommit']);
    expect(methods).not.toContain('txRollback');
  });

  it('t.query().rows() routes through txExecute, not execute', async () => {
    const calls: TxCall[] = [];
    const fc = fakeTxClient(calls);
    const db = new Db(asClient(fc), 'test_db');

    await db.tx(async (t) => {
      await t.query('x').rows();
    });

    const methods = calls.map((c) => c.method);
    expect(methods).toContain('txExecute');
    expect(methods).not.toContain('execute');
  });

  it('t.run() and db.run() produce identical batch shapes', async () => {
    const dbCaptured: Captured[] = [];
    const fc1 = fakeClient(dbCaptured);
    const db = new Db(asClient(fc1), 'test_db');
    await db.run(write.insert('x', [{ a: 1 }]));
    const dbBatch = dbCaptured[0].batch;

    const txCalls: TxCall[] = [];
    const fc2 = fakeTxClient(txCalls);
    const db2 = new Db(asClient(fc2), 'test_db');
    await db2.tx(async (t) => {
      await t.run(write.insert('x', [{ a: 1 }]));
    });
    const txBatch = txCalls[1].args[2]; // [db, handle, batch]

    expect(dbBatch).toEqual(txBatch);
  });
});

// ─── runOne edge cases (TS-T17 review fixes) ─────────────────────────────────

describe('Db.run edge cases', () => {
  /** Fake client whose execute returns a BatchResponse with NO `_` result. */
  function emptyResultClient() {
    return {
      execute: async (): Promise<BatchResponse> => ({
        id: 1 as WireValue,
        results: {},
        execution_plan: [],
        execution_time_us: 0,
      }),
    } as unknown as import('../client.js').ShamirClient;
  }

  it('throws when the server returns no result for the op (no silent undefined)', async () => {
    const db = new Db(emptyResultClient(), 'test_db');
    await expect(db.run(write.insert('x', [{ a: 1 }]))).rejects.toThrow(
      /no result for the operation/,
    );
  });

  it('treats a raw op carrying a non-function `build` key as a raw op (not a builder)', async () => {
    const captured: Captured[] = [];
    const db = new Db(asClient(fakeClient(captured)), 'test_db');
    // A plain wire op that happens to have a `build` string field must NOT be
    // mistaken for a builder (no `.build()` call attempted).
    const rawOp = { insert_into: 'x', values: [{ a: 1 }], build: 'not-a-fn' };
    await db.run(rawOp as unknown as object);
    const sent = (captured[0].batch as { queries: { _: object } }).queries._;
    expect(sent).toEqual(rawOp);
  });
});
