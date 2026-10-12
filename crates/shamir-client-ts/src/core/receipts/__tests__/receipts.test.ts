import { describe, it, expect, vi } from 'vitest';
import { fixture } from '../../visibility/__tests__/fixture.js';
import { insert } from '../../builders/index.js';
import { reconcileReceipts } from '../reconcile.js';
import type { AliasReceipt } from '../types.js';

const BIG = 9007199254740993n; // 2^53 + 1 — first unsafe-integer u64
const MAX_U64 = 18446744073709551615n;

function pendingReceipt(alias: string, repo: string | null, version: bigint): AliasReceipt {
  return {
    alias,
    repo,
    transaction: {
      tx_id: 1,
      status: 'committed',
      commit_version: version,
      materialized: true,
      visibility: {
        state: 'pending',
        visible_through: version - 1n,
        blocking_version: version,
        reason: 'timeout',
      },
    },
    visibility: {
      state: 'pending',
      visible_through: version - 1n,
      blocking_version: version,
      reason: 'timeout',
    },
  };
}

// Oracle: implicit-batch per-alias receipts are committed outcomes scoped
// to the write alias's own repository; pending never resends.
describe('implicit batch receipts (VIS-5b)', () => {
  it('extracts one visible and one pending receipt with their own repos', async () => {
    const f = await fixture();
    try {
      const batch = f.client
        .db('db')
        .batch()
        .add('w1', insert('items', [{ id: 1 }]))
        .add('w2', insert('sessions', [{ id: 2 }], { repo: 'hot' }))
        .build();
      const run = f.client.executeWithReceipts('db', batch, { mode: 'bounded', timeout_ms: 0 });
      const request = await f.nextRequest();
      expect(request).toMatchObject({
        op: 'execute',
        batch: { visibility_wait: { mode: 'bounded', timeout_ms: 0n } },
      });
      // Implicit path: the batch is NOT flipped transactional.
      expect((request.batch as Record<string, unknown>).transactional).toBeFalsy();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            w1: {
              records: [],
              receipt: { tx_id: 7, status: 'committed', commit_version: 43, materialized: true,
                visibility: { state: 'visible' } },
            },
            w2: {
              records: [],
              receipt: { tx_id: 8, status: 'committed', commit_version: BIG, materialized: true,
                visibility: { state: 'pending', visible_through: BIG - 1n, blocking_version: BIG, reason: 'timeout' } },
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts).toHaveLength(2);
      const w1 = receipts.find((r) => r.alias === 'w1');
      const w2 = receipts.find((r) => r.alias === 'w2');
      expect(w1).toMatchObject({ repo: 'main', visibility: { state: 'visible' },
        transaction: { commit_version: 43n } });
      expect(w2).toMatchObject({ repo: 'hot', visibility: { state: 'pending', reason: 'timeout' },
        transaction: { commit_version: BIG } });
      expect(w2?.transaction.commit_version).toBe(BIG);
      expect(f.requests).toHaveLength(1);
    } finally { await f.client.close(); }
  });

  it('a suppressed write alias still yields its row-less receipt', async () => {
    const f = await fixture();
    try {
      const batch = f.client
        .db('db')
        .batch()
        .add('w1', insert('items', [{ id: 1 }]), { returnResult: false })
        .build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            w1: {
              records: [],
              receipt: { tx_id: 7, status: 'committed', commit_version: 4, materialized: true,
                visibility: { state: 'pending', visible_through: 3, blocking_version: 4, reason: 'reopen_required' } },
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts).toHaveLength(1);
      expect(receipts[0]).toMatchObject({ alias: 'w1', repo: 'main',
        visibility: { state: 'pending', visible_through: 3n, blocking_version: 4n, reason: 'reopen_required' } });
    } finally { await f.client.close(); }
  });

  it.each([
    { status: 'committed', visibility: { state: 'pending', visible_through: 3, blocking_version: 5, reason: 'timeout' } },
    { status: 'committed', visibility: undefined },
    { status: 'aborted', reason: 'tx_conflict', visibility: undefined },
  ])('rejects malformed receipt metadata %#', async (receipt) => {
    const f = await fixture();
    try {
      const batch = f.client.db('db').batch().add('w1', insert('items', [{ id: 1 }])).build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: { id: 1, execution_plan: [], execution_time_us: 0,
          results: { w1: { records: [], receipt: { tx_id: 7, ...receipt, materialized: true } } } },
      });
      await expect(run).rejects.toThrow(/malformed (receipt|transaction visibility) metadata/);
      expect(f.requests).toHaveLength(1);
    } finally { await f.client.close(); }
  });

  it('transport timeout does not trigger a second request', async () => {
    vi.useFakeTimers();
    const f = await fixture(50);
    try {
      const batch = f.client.db('db').batch().add('w1', insert('items', [{ id: 1 }])).build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      await vi.advanceTimersByTimeAsync(50);
      await expect(run).rejects.toMatchObject({
        code: 'client_timeout',
        retryable: false,
        acknowledgement: 'unknown',
      });
      expect(f.requests.map((r) => r.op)).toEqual(['execute']);
    } finally { await f.client.close(); vi.useRealTimers(); }
  });

  it('surfaces nested sub-batch and for-each receipts with path aliases and no repo', async () => {
    const f = await fixture();
    try {
      const batch = f.client
        .db('db')
        .batch()
        .add('outer', { batch: { id: 1, queries: { inner: insert('items', [{ id: 1 }]) } } })
        .add('loop', {
          over: [1], bind_row: 'row',
          for_each: { id: 2, queries: { w: insert('items', [{ id: 2 }]) } },
        })
        .build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            // Real server shape (query_runner.rs): sub-batch container has
            // NO own receipt; its `value` is a MAP alias -> QueryResult.
            outer: {
              records: [],
              value: {
                inner: {
                  records: [],
                  receipt: { tx_id: 3, status: 'committed', commit_version: 2, materialized: true,
                    visibility: { state: 'visible' } },
                },
              },
            },
            // for-each container: `value` is a LIST of iteration maps.
            loop: {
              records: [],
              value: [
                { w: { records: [], receipt: { tx_id: 4, status: 'committed', commit_version: 5,
                  materialized: true, visibility: { state: 'visible' } } } },
              ],
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts.map((r) => [r.alias, r.repo])).toEqual([
        ['outer.inner', null],
        ['loop[0].w', null],
      ]);
      expect(receipts[0].transaction.commit_version).toBe(2n);
      expect(receipts[1].visibility).toEqual({ state: 'visible' });
    } finally { await f.client.close(); }
  });

  it('u64::MAX receipt positions survive the wire losslessly', async () => {
    const f = await fixture();
    try {
      const batch = f.client.db('db').batch().add('w1', insert('items', [{ id: 1 }])).build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            w1: {
              records: [],
              receipt: { tx_id: 7, status: 'committed', commit_version: MAX_U64, snapshot_version: 1,
                materialized: true,
                visibility: { state: 'pending', visible_through: MAX_U64 - 1n, blocking_version: MAX_U64,
                  reason: 'timeout' } },
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts[0].transaction.commit_version).toBe(MAX_U64);
      expect(receipts[0].visibility).toEqual({ state: 'pending',
        visible_through: MAX_U64 - 1n, blocking_version: MAX_U64, reason: 'timeout' });
      expect(f.requests).toHaveLength(1);
    } finally { await f.client.close(); }
  });

  it('maps the fail-closed Bounded rejection to its typed error without retrying', async () => {
    const f = await fixture();
    try {
      const batch = f.client.db('db').batch().add('w1', insert('items', [{ id: 1 }])).build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      // Real server code for receipt-less Call/admin writes under Bounded
      // (engine/.../receipts/policy.rs): code "validation".
      f.respond({ kind: 'error', code: 'validation', message: 'bounded visibility is unsupported' });
      await expect(run).rejects.toMatchObject({ code: 'validation', retryable: false });
      expect(f.requests.map((r) => r.op)).toEqual(['execute']);
    } finally { await f.client.close(); }
  });

  it('reconcile sends the real WaitForVisibility wire requests and refuses guesses', async () => {
    const f = await fixture();
    try {
      const receipts = [
        pendingReceipt('a', 'main', 5n),
        pendingReceipt('b', 'main', BIG),
        pendingReceipt('c', 'hot', 3n),
        { ...pendingReceipt('d', 'hot', 4n), visibility: { state: 'visible' as const } },
      ];
      // PRODUCTION reconciliation: grouping (per repository, max pending
      // position, one observation per repository) happens inside
      // reconcileReceipts; the observer drives the CLIENT so the mock
      // socket records the actual wire objects written.
      const observed: Array<{ repo: string; version: bigint }> = [];
      const observations = await reconcileReceipts(
        receipts,
        (repo, version) => {
          const requestDone = f.client.waitForVisibility('db', repo, version, {
            mode: 'bounded', timeout_ms: 0,
          });
          // Await the request ARRIVING on the wire, assert it, answer it —
          // then await the observation. (Responding only after requestDone
          // resolves would deadlock: the response IS what resolves it.)
          return f.nextRequest().then((request) => {
            expect(request).toMatchObject({
              op: 'wait_for_visibility', query_version: 3, db: 'db', repo, version,
              visibility_wait: { mode: 'bounded', timeout_ms: 0n },
            });
            observed.push({ repo, version });
            f.respond({ kind: 'visibility_observed', version, visibility: { state: 'visible' } });
            return requestDone.then((state) => {
              expect(state).toEqual({ state: 'visible' });
              return state;
            });
          });
        },
      );
      expect(observed).toEqual([{ repo: 'main', version: BIG }, { repo: 'hot', version: 3n }]);
      expect(observations).toHaveLength(2);
      await expect(
        reconcileReceipts([pendingReceipt('nested', null, 9n)], async () => ({ state: 'visible' })),
      ).rejects.toThrow(/no derivable repository/);
    } finally { await f.client.close(); }
  });

  it('nested receipts accept decimal-string positions above 2^53 and i64::MAX', async () => {
    const f = await fixture();
    try {
      const batch = f.client
        .db('db')
        .batch()
        .add('outer', { batch: { id: 1, queries: { w: insert('items', [{ id: 1 }]) } } })
        .build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            outer: {
              records: [],
              value: {
                w: {
                  records: [],
                  // Big positions travel as DECIMAL STRINGS on the wire.
                  receipt: { tx_id: 7, status: 'committed',
                    commit_version: '18446744073709551615', snapshot_version: '9007199254740993',
                    materialized: true,
                    visibility: { state: 'pending', visible_through: '18446744073709551614',
                      blocking_version: '18446744073709551615', reason: 'timeout' } },
                },
              },
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts[0].alias).toBe('outer.w');
      expect(receipts[0].repo).toBeNull();
      expect(receipts[0].transaction.commit_version).toBe(MAX_U64);
      expect(receipts[0].transaction.tx_id).toBe(7);
      expect(receipts[0].visibility).toEqual({ state: 'pending',
        visible_through: MAX_U64 - 1n, blocking_version: MAX_U64, reason: 'timeout' });
    } finally { await f.client.close(); }
  });

  it('a READ result whose application value member carries a receipt yields NO receipt', async () => {
    const f = await fixture();
    try {
      // Non-container READ entry (a real query op through the builder).
      const batch = f.client.db('db').batch().add('w1', f.client.db('db').query('items')).build();
      const run = f.client.executeWithReceipts('db', batch);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            w1: {
              records: [],
              // Application data shaped like the old walker's target: a
              // value MAP whose MEMBER carries its own valid-looking
              // receipt. Non-container ops are never inspected, so this
              // must fabricate NOTHING.
              value: {
                phantom: {
                  records: [],
                  receipt: { tx_id: 9, status: 'committed', commit_version: 1,
                    materialized: true, visibility: { state: 'visible' } },
                },
              },
            },
          },
        },
      });
      const { receipts } = await run;
      expect(receipts).toEqual([]);
    } finally { await f.client.close(); }
  });
});

// Oracle: `changes_since` is a settlement sample; u64 positions stay
// lossless end to end (2^53+1 and u64::MAX) on the MessagePack path.
describe('typed changesSince (VIS-5b)', () => {
  it.each([BIG, MAX_U64])('keeps cursor %s and visible_through lossless', async (cursor) => {
    const f = await fixture();
    try {
      const run = f.client.changesSince('db', 'archive', cursor, 7);
      expect(await f.nextRequest()).toMatchObject({
        op: 'execute',
        batch: { queries: { changes_since: { changes_since: cursor, repo: 'archive', limit: 7 } } },
      });
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            changes_since: {
              records: [{
                changes_since: cursor,
                gap_at: null,
                visible_through: MAX_U64,
                events: [{
                  repo: 'archive', commit_version: cursor, tx_id: 7, actor: { User: 42 },
                  timestamp_ns: BIG,
                  changes: [
                    // bin wire form (framing Uint8Array).
                    { table: 'items', key: new Uint8Array([1, 2]), op: 'Put', value: new Uint8Array([9]) },
                    // array-of-byte-integers wire form (the optional value
                    // adapter of the server emits a fixarray, NOT bin).
                    { table: 'logs', key: [3, 4], op: 'Delete', value: null },
                  ],
                }],
              }],
            },
          },
        },
      });
      const result = await run;
      expect(result.changes_since).toBe(cursor);
      expect(result.gap_at).toBeNull();
      expect(result.visible_through).toBe(MAX_U64);
      expect(result.events).toHaveLength(1);
      expect(result.events[0]).toMatchObject({ repo: 'archive', commit_version: cursor, tx_id: 7n,
        timestamp_ns: BIG,
        changes: [
          { table: 'items', op: 'Put' },
          { table: 'logs', op: 'Delete' },
        ] });
      expect(result.events[0].changes[0].key).toEqual(new Uint8Array([1, 2]));
      expect(result.events[0].changes[0].value).toEqual(new Uint8Array([9]));
      expect(result.events[0].changes[1].key).toEqual(new Uint8Array([3, 4]));
      expect(result.events[0].changes[1].value).toBeNull();
    } finally { await f.client.close(); }
  });

  it('decodes a pre-VIS-4c payload without visible_through as null', async () => {
    const f = await fixture();
    try {
      const run = f.client.changesSince('db', 'main', 41);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: {
          id: 1,
          execution_plan: [],
          execution_time_us: 0,
          results: {
            changes_since: { records: [{ changes_since: 41, gap_at: 38, events: [] }] },
          },
        },
      });
      const result = await run;
      expect(result).toEqual({ changes_since: 41n, events: [], gap_at: 38n, visible_through: null });
    } finally { await f.client.close(); }
  });

  it('number cursors below 2^53 keep compiling and travelling', async () => {
    const f = await fixture();
    try {
      const run = f.client.changesSince('db', 'main', 41);
      await f.nextRequest();
      f.respond({
        kind: 'batch',
        response: { id: 1, execution_plan: [], execution_time_us: 0,
          results: { changes_since: { records: [{ changes_since: 41, events: [] }] } } },
      });
      expect((await run).changes_since).toBe(41n);
    } finally { await f.client.close(); }
  });
});
