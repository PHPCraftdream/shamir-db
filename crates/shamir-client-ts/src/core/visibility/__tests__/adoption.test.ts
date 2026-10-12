import { describe, it, expect, vi } from 'vitest';
import { fixture } from './fixture.js';
import { VisibilityError } from '../errors.js';

// Oracle: protocol v3 receipts are committed independently of visibility/durability.
describe('visibility wire adoption', () => {
  it.each([9007199254740993n, 18446744073709551615n])('retains u64 %s and callback value without replay', async (version) => {
    const f = await fixture();
    try {
      const result = f.client.db('db').tx(async () => 'value', {
        visibilityWait: { mode: 'bounded', timeout_ms: 0 },
      });
      expect((await f.nextRequest()).op).toBe('tx_begin');
      f.respond({ kind: 'tx_opened', tx_handle: 7, snapshot_version: 1, isolation: 'snapshot' });
      const commit = await f.nextRequest();
      expect(commit).toMatchObject({ op: 'tx_commit', query_version: 3,
        visibility_wait: { mode: 'bounded', timeout_ms: 0n } });
      f.respond({ kind: 'tx_committed', transaction: {
        tx_id: 7, status: 'committed', commit_version: version, snapshot_version: 1,
        materialized: false, error: 'wal_durability', visibility: {
          state: 'pending', visible_through: version - 2n, blocking_version: version - 1n, reason: 'timeout',
        },
      } });
      const receipt = await result;
      expect(receipt).toMatchObject({ status: 'committed', state: 'pending', value: 'value',
        transaction: { commit_version: version, error: 'wal_durability' } });
      expect(receipt.visibility).toMatchObject({ visible_through: version - 2n,
        blocking_version: version - 1n, reason: 'timeout' });
      expect(f.requests.map((r) => r.op)).toEqual(['tx_begin', 'tx_commit']);
      const observed = f.client.waitForVisibility('db', 'main', version, { mode: 'bounded', timeout_ms: 0 });
      expect(await f.nextRequest()).toMatchObject({ op: 'wait_for_visibility', version, query_version: 3 });
      f.respond({ kind: 'visibility_observed', version, visibility: { state: 'visible' } });
      expect(await observed).toEqual({ state: 'visible' });
    } finally { await f.client.close(); }
  });

  it('transactional execute options and receipt survive the wire', async () => {
    const f = await fixture();
    try {
      // Query construction uses the public builder; the fixture tests response wire metadata.
      const batch = f.client.db('db').batch().transactional().build();
      const result = f.client.execute('db', batch, { mode: 'bounded', timeout_ms: 0 });
      expect(await f.nextRequest()).toMatchObject({ op: 'execute', batch: {
        transactional: true, visibility_wait: { mode: 'bounded', timeout_ms: 0n },
      } });
      f.respond({ kind: 'batch', response: { id: 1, results: {}, execution_plan: [], execution_time_us: 0,
        transaction: { tx_id: 7, status: 'committed', commit_version: 4, materialized: true,
          visibility: { state: 'pending', reason: 'reopen_required', visible_through: 1, blocking_version: 2 } } } });
      expect((await result).transaction).toMatchObject({ commit_version: 4n,
        visibility: { state: 'pending', visible_through: 1n, blocking_version: 2n } });
      expect(f.requests).toHaveLength(1);
    } finally { await f.client.close(); }
  });

  it('visible commits retain the callback and normalize small wire positions', async () => {
    const f = await fixture();
    try {
      const result = f.client.db('db').tx(async () => 123);
      await f.nextRequest();
      f.respond({ kind: 'tx_opened', tx_handle: 7, snapshot_version: 1, isolation: 'snapshot' });
      await f.nextRequest();
      f.respond({ kind: 'tx_committed', transaction: { tx_id: 7, status: 'committed',
        commit_version: 2, snapshot_version: 1, materialized: true, visibility: { state: 'visible' } } });
      expect(await result).toMatchObject({ status: 'committed', state: 'visible', value: 123,
        transaction: { commit_version: 2n, snapshot_version: 1n } });
      expect(f.requests.map((r) => r.op)).toEqual(['tx_begin', 'tx_commit']);
    } finally { await f.client.close(); }
  });

  it.each(['tx_conflict', 'wal_requires_reopen'])('preserves abort classification %s', async (reason) => {
    const f = await fixture();
    try {
      const result = f.client.db('db').tx(async () => 1);
      const rejected = expect(result).rejects.toMatchObject({ code: reason, transaction: { reason, status: 'aborted' } });
      await f.nextRequest();
      f.respond({ kind: 'tx_opened', tx_handle: 7, snapshot_version: 1, isolation: 'snapshot' });
      await f.nextRequest();
      f.respond({ kind: 'tx_committed', transaction: { tx_id: 7, status: 'aborted', reason, materialized: false } });
      await rejected;
      expect(f.requests.map((r) => r.op)).toEqual(['tx_begin', 'tx_commit']);
    } finally { await f.client.close(); }
  });

  it('commit acknowledgement timeout neither rolls back nor resends', async () => {
    vi.useFakeTimers();
    const f = await fixture(50);
    try {
      const result = f.client.db('db').tx(async () => 1);
      const rejected = expect(result).rejects.toMatchObject({ code: 'client_timeout', retryable: false, acknowledgement: 'unknown' });
      await f.nextRequest();
      f.respond({ kind: 'tx_opened', tx_handle: 7, snapshot_version: 1, isolation: 'snapshot' });
      await f.nextRequest();
      await vi.advanceTimersByTimeAsync(50);
      await rejected;
      expect(f.requests.map((r) => r.op)).toEqual(['tx_begin', 'tx_commit']);
    } finally { await f.client.close(); vi.useRealTimers(); }
  });

  it.each(['as_of_visibility_timeout', 'as_of_future_version', 'as_of_reopen_required',
    'visibility_future_version', 'visibility_wait_invalid', 'write_committed_visibility_pending'])('maps %s from real response fields', async (code) => {
    const f = await fixture();
    try {
      const result = f.client.waitForVisibility('db', 'main', 1n, { mode: 'bounded', timeout_ms: 0 });
      const rejected = expect(result).rejects.toBeInstanceOf(VisibilityError);
      await f.nextRequest();
      f.respond({ kind: code === 'as_of_visibility_timeout' ? code : 'error', code, message: 'detail',
        ...(code === 'as_of_visibility_timeout' ? { requested: 9007199254740993n, visible_through: 1, blocking_version: 2 } : {}) });
      await rejected;
      const error = await result.catch((e: unknown) => e);
      expect(error).toMatchObject({ code, retryable: false });
      if (code === 'as_of_visibility_timeout') expect(error).toMatchObject({ requested: 9007199254740993n, visible_through: 1n, blocking_version: 2n });
    } finally { await f.client.close(); }
  });

  it.each([undefined, { state: 'pending', reason: 'timeout', visible_through: 3, blocking_version: 5 },
    { state: 'pending', reason: 'invalid', visible_through: 3, blocking_version: 4 }])('rejects malformed metadata', async (visibility) => {
    const f = await fixture();
    try {
      const result = f.client.txCommit('db', 7);
      const rejected = expect(result).rejects.toThrow();
      await f.nextRequest();
      f.respond({ kind: 'tx_committed', transaction: { tx_id: 7, status: 'committed', materialized: true, visibility } });
      await rejected;
      expect(f.requests).toHaveLength(1);
    } finally { await f.client.close(); }
  });
});
