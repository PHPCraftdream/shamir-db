/**
 * `CursorIterator` (FG-5d) — idiomatic async iterator wrapper over the
 * server-side cursor wire protocol (FG-5a/FG-5b), the TS-side equivalent of
 * the Rust SDK's `CursorStream` (FG-5c, `crates/shamir-client/src/cursor_stream.rs`).
 *
 * Implements `AsyncIterableIterator<Record<string, WireValue>>` so callers
 * consume it with `for await (const record of cursor) { ... }` — one record
 * at a time, NOT one page at a time, matching FG-5c's per-record granularity
 * and fixing the "materializes as array" problem on the TS client side too.
 *
 * Internally issues `create_cursor` on the first `next()` call, then
 * `fetch_next` whenever the current page's buffer drains, until the server
 * reports `has_more: false`.
 *
 * # Errors
 *
 * Unlike the Rust SDK (which wraps every yielded item in a `Result`), TS
 * errors propagate as THROWN exceptions — `sendRequest` (backed by
 * `ShamirClient.sendDbRequest`) already converts any `DbResponse` with
 * `kind === 'error'` into a thrown {@link ShamirDbError} (see
 * `client.ts`'s `readLoop`), so every cursor error code FG-5b defined
 * (`cursor_not_found` / `cursor_expired` / `cursor_limit_exceeded` /
 * `cursor_temporal_not_supported`) surfaces this way automatically. A
 * `for await` loop that hits one throws naturally, exactly like a sync
 * generator throwing mid-iteration. There is deliberately no `Result`-style
 * wrapped-error item type here — that would be unidiomatic in TS.
 *
 * # Cleanup on early exit — a real improvement over the Rust SDK, not just parity
 *
 * Rust's `Drop` is synchronous, so FG-5c's `CursorStream` could not run a
 * network call when a stream was dropped early and had to rely solely on
 * the server-side idle-timeout (60s default, FG-5b) as the backstop for an
 * abandoned stream.
 *
 * TypeScript's `AsyncIterator.return()` is itself `async` and IS awaited by
 * the JS runtime on every `for await...of` early exit (`break`, `return`,
 * or an exception thrown inside the loop body) — per the language's own
 * async-iterator protocol (see the `Symbol.asyncIterator` proposal /
 * ECMA-262 `IteratorClose`). This lets {@link CursorIterator.return} send
 * `cancel_cursor` DETERMINISTICALLY on every early exit, which is strictly
 * better than the Rust SDK's story rather than merely equivalent — so this
 * class does that instead of punting to the idle-timeout backstop by
 * default.
 *
 * The idle-timeout backstop still matters for exactly one remaining case:
 * a caller that holds the iterator without ever using a `for await` loop
 * (never calls `next()` again, never calls `.close()`/`.return()`, simply
 * drops the reference). JS has no deterministic destructors, so THAT
 * specific case still relies on the server-side idle-timeout reaper, same
 * as the Rust SDK.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ReadQuery } from './types/query.js';
import type { WireValue } from './types/write.js';
import type { CursorId, CursorPageResponse } from './types/cursor.js';
import { createCursor, fetchNext, cancelCursor } from './builders/cursor.js';

/** Something that has a `.build()` method returning a `ReadQuery`. */
export interface QueryBuildable {
  build(): ReadQuery;
}

/** A single materialized record — one page's worth of buffered rows. */
type CursorRecord = Record<string, WireValue>;

/**
 * Round-trips one wire request object and returns the decoded `DbResponse`.
 * Injected by the caller (dependency injection) so this module never needs
 * visibility into `ShamirClient`'s private `sendDbRequest` — see
 * `ShamirClient.streamCursor` for the bound closure that supplies this.
 */
export type SendCursorRequest = (req: object) => Promise<Record<string, unknown>>;

/**
 * Pull-based async iterator over a server-side cursor. Obtained via
 * {@link ShamirClient.streamCursor} or `Db.cursor`.
 */
export class CursorIterator implements AsyncIterableIterator<CursorRecord> {
  /** Server-assigned cursor id, set once `create_cursor` succeeds. */
  private _cursorId: CursorId | undefined;
  /** Not-yet-yielded records from the most recently fetched page. */
  private buffer: CursorRecord[] = [];
  /** Read offset into `buffer` — see `position`'s own doc comment (P-5). */
  private position = 0;
  /** Whether another `fetch_next` should be attempted once `buffer` drains. */
  private hasMore = true;
  /** Set once the cursor is known to be closed (drained or explicitly closed). */
  private done = false;
  /**
   * CR-C1 (R-10): internal serialization chain for `next()`. Two overlapping
   * calls to `next()` (before the first resolves — e.g. a caller that
   * doesn't `await` each iteration, or manually drives `.next()` from two
   * spots) would otherwise both reach `sendRequest` concurrently; on the
   * FIRST call both could race to issue `createCursor`, leaking the loser's
   * cursor server-side until idle-timeout (nothing ever cancels it, since
   * the caller only ends up holding the winner's `_cursorId`). Chaining
   * every call's work onto this promise makes a second overlapping call
   * queue behind the first instead of racing it.
   */
  private pending: Promise<IteratorResult<CursorRecord>> = Promise.resolve({
    done: false,
    value: undefined as unknown as CursorRecord,
  });

  constructor(
    private readonly sendRequest: SendCursorRequest,
    private readonly db: string,
    private readonly query: ReadQuery | QueryBuildable,
    private readonly pageSize: number,
  ) {}

  /**
   * The server-assigned cursor id, once known. `undefined` until the first
   * page has been fetched (the underlying `create_cursor` round-trip has
   * not completed yet — e.g. `next()` was never called, or the very first
   * call is still in flight). Exposed primarily for tests that need to
   * drive a raw follow-up request against the same id, mirroring FG-5c's
   * Rust `CursorStream::cursor_id()` accessor.
   */
  get cursorId(): CursorId | undefined {
    return this._cursorId;
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<CursorRecord> {
    return this;
  }

  async next(): Promise<IteratorResult<CursorRecord>> {
    // CR-C1 (R-10): chain onto the pending promise rather than running
    // `doNext()` immediately — serializes overlapping `next()` calls so a
    // second call queues behind the first instead of racing it (see
    // `pending`'s doc comment). `.then()` runs `doNext()` only once the
    // previous call's work (success OR failure) has settled; the NEW
    // `pending` becomes this call's own result so a THIRD overlapping call
    // queues behind this one in turn.
    const result = this.pending.then(
      () => this.doNext(),
      () => this.doNext(),
    );
    this.pending = result;
    return result;
  }

  /**
   * The actual per-call `next()` work, run serialized behind `pending`. Uses
   * a loop (not recursion) over empty-but-`hasMore` pages — a legal but
   * pathological server response shape (e.g. every row on a page filtered
   * client-invisible) — so a pathological "every page comes back empty but
   * has_more stays true" server bug cannot grow this call's stack
   * unboundedly (should never happen given CR-B4's `has_more` fix landed
   * server-side, but a client shouldn't rely on server correctness for its
   * own stack safety).
   */
  private async doNext(): Promise<IteratorResult<CursorRecord>> {
    if (this.done) {
      return { done: true, value: undefined };
    }

    for (;;) {
      if (this.position >= this.buffer.length) {
        if (this._cursorId === undefined) {
          // First call: open the cursor.
          const resp = await this.sendRequest(createCursor(this.db, this.query, this.pageSize));
          this.applyPage(resp);
        } else if (this.hasMore) {
          const resp = await this.sendRequest(fetchNext(this._cursorId, this.pageSize));
          this.applyPage(resp);
        } else {
          this.done = true;
          return { done: true, value: undefined };
        }
      }

      if (this.position < this.buffer.length) {
        const value = this.buffer[this.position];
        this.position += 1;
        return { done: false, value };
      }
      // Buffer drained again immediately (e.g. a legal-but-pathological
      // empty page with has_more still true) — loop to try the next page
      // rather than yielding a bogus `done` or recursing.
    }
  }

  /**
   * Unwrap a `create_cursor`/`fetch_next` response into the buffer + more
   * flag. Throws on an unexpected `kind`, matching `ShamirClient.execute`'s
   * existing convention — never silently swallowed into `done: true`.
   */
  private applyPage(resp: Record<string, unknown>): void {
    if (resp.kind !== 'cursor_page') {
      throw new Error(
        `unexpected DbResponse kind for cursor page fetch: ${(resp.kind as string) ?? 'missing'}`,
      );
    }
    const page = resp as unknown as CursorPageResponse;
    this._cursorId = page.cursor_id;
    this.buffer = page.page.records;
    // A fresh page always starts unread — reset the index-based read
    // position (P-5: `buffer.shift()` was O(n) per read; an index avoids
    // re-indexing the whole array on every record).
    this.position = 0;
    this.hasMore = page.has_more;
  }

  /**
   * Send `cancel_cursor` deterministically (see the module doc comment for
   * why this is safe/beneficial in TS unlike Rust's `Drop`). A no-op if no
   * cursor was ever created (`next()` was never called) or the cursor is
   * already known to be done (drained or already closed).
   *
   * Called automatically by the JS runtime on every `for await...of` early
   * exit (`break`, `return`, or an exception in the loop body) per the
   * async-iterator protocol — INCLUDING as part of unwinding an exception
   * thrown from inside the loop body (JS's `IteratorClose` calls `.return()`
   * during exception propagation, not just on a clean `break`). An
   * unexpected `cancel_cursor` response kind is therefore swallowed (logged,
   * not thrown): throwing here would REPLACE/mask whatever exception the
   * loop body itself threw, which is almost always the wrong debugging
   * experience for a caller chasing down their own bug, not a cursor
   * protocol mismatch.
   *
   * N-9 (CR-D5, #786): a manual driver (calling `.next()` and `.return()`
   * directly rather than via `for await...of`, which always awaits each
   * `next()` before the next call) can overlap an in-flight `next()` with a
   * `return()` call. Without waiting for `pending` first, this method's own
   * cleanup (`buffer = []`, `position = 0`) could run BEFORE the in-flight
   * `doNext()` finishes and calls `applyPage`, which would repopulate
   * `buffer`/`position`/`hasMore` right after `return()` just cleared them —
   * silently undoing the cancellation from the caller's point of view.
   * Awaiting `this.pending` first serializes against that in-flight call,
   * mirroring `next()`'s own chaining convention (see `pending`'s doc
   * comment above), so this method's cleanup always runs LAST.
   *
   * One related case is deliberately left as-is, not fixed: `return()`
   * called while the FIRST `next()` is still in flight (before
   * `_cursorId` is even known yet) skips the server-side `cancel_cursor`
   * call entirely below (the `this._cursorId !== undefined` guard), leaving
   * that cursor to the idle-timeout backstop — this mirrors the Rust SDK's
   * own documented reliance on the idle-timeout reaper for a `Drop`-based
   * early abandonment, so it's an accepted, understood gap rather than an
   * oversight.
   */
  async return(): Promise<IteratorResult<CursorRecord>> {
    // Serialize against any in-flight next()/doNext() — see the doc comment
    // above. Swallow a rejection the same way `next()`'s own `.then(...,
    // () => this.doNext())` does: this call only cares that the prior work
    // has SETTLED before its own cleanup runs, not what it resolved to.
    await this.pending.catch(() => undefined);

    if (!this.done && this._cursorId !== undefined) {
      this.done = true;
      try {
        const resp = await this.sendRequest(cancelCursor(this._cursorId));
        if (resp.kind !== 'cursor_closed') {
          // eslint-disable-next-line no-console
          console.warn(
            `CursorIterator: unexpected DbResponse kind for cancel_cursor: ${
              (resp.kind as string) ?? 'missing'
            }`,
          );
        }
      } catch (err) {
        // Same rationale: a cancel-cursor failure during IteratorClose must
        // not mask whatever the loop body/original iteration was doing.
        // eslint-disable-next-line no-console
        console.warn('CursorIterator: cancel_cursor request failed during return()', err);
      }
    }
    this.done = true;
    this.buffer = [];
    this.position = 0;
    return { done: true, value: undefined };
  }

  /**
   * Explicit alias for {@link CursorIterator.return}, for callers driving
   * the iterator manually outside a `for await` loop (no implicit
   * `IteratorClose` call applies there).
   */
  async close(): Promise<void> {
    await this.return();
  }
}
