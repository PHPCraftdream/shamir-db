import { describe, it, expect, vi } from 'vitest';
import { CursorIterator, type SendCursorRequest } from '../cursor-iterator.js';
import type { ReadQuery } from '../types/query.js';

/** Minimal `ReadQuery` fixture — the iterator never inspects its shape. */
const query = { from: { repo: 'main', table: 'items' } } as unknown as ReadQuery;

/** Build a `cursor_page` response object with `n` numbered records. */
function page(
  cursorId: number,
  records: Array<Record<string, unknown>>,
  hasMore: boolean,
): Record<string, unknown> {
  return {
    kind: 'cursor_page',
    cursor_id: cursorId,
    page: { records, stats: null },
    has_more: hasMore,
  };
}

describe('CursorIterator', () => {
  // -------------------------------------------------------------------
  // R-10: `.next()` serialization — two overlapping calls must not both
  // reach `sendRequest` with `create_cursor` (which would leak the
  // loser's cursor server-side).
  // -------------------------------------------------------------------

  it('serializes two overlapping next() calls into exactly one create_cursor request', async () => {
    let createCursorCalls = 0;
    let resolveFirst!: (v: Record<string, unknown>) => void;

    const sendRequest: SendCursorRequest = vi.fn((req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        createCursorCalls += 1;
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const iter = new CursorIterator(sendRequest, 'app', query, 10);

    // Fire two overlapping next() calls BEFORE the first create_cursor
    // round-trip resolves.
    const p1 = iter.next();
    const p2 = iter.next();

    // `next()` chains its work onto a promise (see `pending`'s doc comment
    // in cursor-iterator.ts), so `doNext()` runs as a microtask rather than
    // synchronously inside `next()` itself — let the microtask queue drain
    // once before asserting how many `sendRequest` calls have happened so
    // far. This does NOT weaken the assertion: if `next()` raced instead of
    // serializing, BOTH calls' `doNext()` would still have reached
    // `sendRequest` by the time this microtask flush completes, since
    // nothing here awaits the actual (still-unresolved) `create_cursor`
    // response.
    await Promise.resolve();
    await Promise.resolve();

    // Only one create_cursor request should have been issued so far — the
    // second next() must be queued behind the first, not racing it.
    expect(createCursorCalls).toBe(1);

    // Resolve the create_cursor round-trip with a 2-row page (has_more:
    // false, so no fetch_next is needed to satisfy both next() calls).
    resolveFirst(page(1, [{ id: 'a' }, { id: 'b' }], false));

    const r1 = await p1;
    const r2 = await p2;

    expect(createCursorCalls).toBe(1);
    expect(r1).toEqual({ done: false, value: { id: 'a' } });
    expect(r2).toEqual({ done: false, value: { id: 'b' } });
  });

  it('processes many overlapping next() calls in order without racing create_cursor', async () => {
    const sendRequest: SendCursorRequest = vi.fn(async (req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        return page(1, [{ v: 0 }, { v: 1 }, { v: 2 }], false);
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const iter = new CursorIterator(sendRequest, 'app', query, 10);

    const results = await Promise.all([iter.next(), iter.next(), iter.next(), iter.next()]);

    expect(results[0]).toEqual({ done: false, value: { v: 0 } });
    expect(results[1]).toEqual({ done: false, value: { v: 1 } });
    expect(results[2]).toEqual({ done: false, value: { v: 2 } });
    expect(results[3]).toEqual({ done: true, value: undefined });

    const createCursorCalls = (sendRequest as ReturnType<typeof vi.fn>).mock.calls.filter(
      (call: unknown[]) => (call[0] as { op: string }).op === 'create_cursor',
    ).length;
    expect(createCursorCalls).toBe(1);
  });

  // -------------------------------------------------------------------
  // R-10: `.return()` must not throw on an unexpected response kind.
  // -------------------------------------------------------------------

  it('return() does not throw on an unexpected cancel_cursor response kind', async () => {
    const sendRequest: SendCursorRequest = vi.fn(async (req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        return page(7, [{ id: 'a' }], true);
      }
      if (r.op === 'cancel_cursor') {
        // Malformed/unexpected response kind.
        return { kind: 'something_else' };
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      const iter = new CursorIterator(sendRequest, 'app', query, 10);
      await iter.next(); // opens the cursor

      const result = await iter.return();

      expect(result).toEqual({ done: true, value: undefined });
      expect(warnSpy).toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
    }
  });

  it('return() does not throw when the cancel_cursor request itself rejects', async () => {
    const sendRequest: SendCursorRequest = vi.fn(async (req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        return page(7, [{ id: 'a' }], true);
      }
      if (r.op === 'cancel_cursor') {
        throw new Error('network error');
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      const iter = new CursorIterator(sendRequest, 'app', query, 10);
      await iter.next();

      await expect(iter.return()).resolves.toEqual({ done: true, value: undefined });
      expect(warnSpy).toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
    }
  });

  // -------------------------------------------------------------------
  // N-9 (CR-D5, #786): return() must serialize behind an in-flight next()
  // so a late-resolving fetch_next cannot repopulate the buffer AFTER
  // return() already cleared it (manual-driving overlap — `for await...of`
  // never overlaps a next() with a return() this way).
  // -------------------------------------------------------------------

  it('return() overlapping an in-flight next() does not let the buffer get repopulated', async () => {
    let resolveFetchNext!: (v: Record<string, unknown>) => void;

    const sendRequest: SendCursorRequest = vi.fn((req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        // has_more: true so a follow-up next() issues a real fetch_next
        // round-trip that we can hold open.
        return Promise.resolve(page(7, [{ id: 'a' }], true));
      }
      if (r.op === 'fetch_next') {
        return new Promise((resolve) => {
          resolveFetchNext = resolve;
        });
      }
      if (r.op === 'cancel_cursor') {
        return Promise.resolve({ kind: 'cursor_closed' });
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const iter = new CursorIterator(sendRequest, 'app', query, 10);

    // Drain the first (already-buffered) record so the SECOND next() call
    // below is the one that actually reaches out for fetch_next.
    const first = await iter.next();
    expect(first).toEqual({ done: false, value: { id: 'a' } });

    // Fire an overlapping next() (issues fetch_next, held open above) and
    // return() BEFORE that fetch_next round-trip resolves — the manual-
    // driving race this task closes.
    const nextPromise = iter.next();
    const returnPromise = iter.return();

    // `next()` chains its work onto `pending` (see cursor-iterator.ts's doc
    // comment), so `doNext()` — and its `sendRequest(fetch_next)` call —
    // runs as a microtask rather than synchronously inside `next()` itself.
    // Flush the microtask queue so `resolveFetchNext` is actually assigned
    // before we call it.
    await Promise.resolve();
    await Promise.resolve();

    // Let the in-flight fetch_next resolve with a NEW page — if return()'s
    // cleanup ran before this, the buffer would be repopulated with these
    // rows right after return() supposedly cleared it.
    resolveFetchNext(page(7, [{ id: 'late' }], false));

    const nextResult = await nextPromise;
    const returnResult = await returnPromise;

    // The queued next() still resolves with the in-flight page's data (it
    // was already promised to the caller before return() ran) — return()
    // does not retroactively cancel a next() call that already started.
    expect(nextResult).toEqual({ done: false, value: { id: 'late' } });
    expect(returnResult).toEqual({ done: true, value: undefined });

    // The core guarantee: once return() has SETTLED, the buffer must be
    // empty and the iterator done — the late fetch_next resolution must not
    // have left anything behind for a LATER next() call to hand out.
    const afterReturn = await iter.next();
    expect(afterReturn).toEqual({ done: true, value: undefined });

    // cancel_cursor must actually have been sent — return() didn't skip its
    // own work despite the overlap.
    const cancelCalls = (sendRequest as ReturnType<typeof vi.fn>).mock.calls.filter(
      (call: unknown[]) => (call[0] as { op: string }).op === 'cancel_cursor',
    );
    expect(cancelCalls.length).toBe(1);
  });

  // -------------------------------------------------------------------
  // P-5 (TS): repeated-empty-page loop (not recursion) + index-based
  // buffer reads still return records in order.
  // -------------------------------------------------------------------

  it('proceeds past multiple consecutive empty-but-has_more pages without stack overflow', async () => {
    let call = 0;
    const EMPTY_PAGES = 5000; // enough to blow a naive recursive stack.

    const sendRequest: SendCursorRequest = vi.fn(async (req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        call += 1;
        return page(1, [], true);
      }
      if (r.op === 'fetch_next') {
        call += 1;
        if (call <= EMPTY_PAGES) {
          return page(1, [], true);
        }
        return page(1, [{ v: 'final' }], false);
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const iter = new CursorIterator(sendRequest, 'app', query, 10);

    const result = await iter.next();
    expect(result).toEqual({ done: false, value: { v: 'final' } });
  });

  it('returns records in order after the shift-to-index buffer change, across multiple pages', async () => {
    const sendRequest: SendCursorRequest = vi.fn(async (req: object) => {
      const r = req as { op: string };
      if (r.op === 'create_cursor') {
        return page(1, [{ v: 0 }, { v: 1 }, { v: 2 }], true);
      }
      if (r.op === 'fetch_next') {
        return page(1, [{ v: 3 }, { v: 4 }], false);
      }
      throw new Error(`unexpected op in test: ${r.op}`);
    });

    const iter = new CursorIterator(sendRequest, 'app', query, 3);

    const collected: unknown[] = [];
    for (let i = 0; i < 5; i++) {
      const r = await iter.next();
      expect(r.done).toBe(false);
      collected.push((r.value as { v: number }).v);
    }
    const last = await iter.next();
    expect(last.done).toBe(true);

    expect(collected).toEqual([0, 1, 2, 3, 4]);
  });
});
