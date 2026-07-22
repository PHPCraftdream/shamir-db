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
  /** Whether another `fetch_next` should be attempted once `buffer` drains. */
  private hasMore = true;
  /** Set once the cursor is known to be closed (drained or explicitly closed). */
  private done = false;

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
    if (this.done) {
      return { done: true, value: undefined };
    }

    if (this.buffer.length === 0) {
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

    const value = this.buffer.shift();
    if (value === undefined) {
      // Buffer drained again immediately (e.g. a legal-but-pathological
      // empty page with has_more still true) — recurse to try the next
      // page rather than yielding a bogus `done`.
      return this.next();
    }
    return { done: false, value };
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
   * async-iterator protocol.
   */
  async return(): Promise<IteratorResult<CursorRecord>> {
    if (!this.done && this._cursorId !== undefined) {
      this.done = true;
      const resp = await this.sendRequest(cancelCursor(this._cursorId));
      if (resp.kind !== 'cursor_closed') {
        throw new Error(
          `unexpected DbResponse kind for cancel_cursor: ${(resp.kind as string) ?? 'missing'}`,
        );
      }
    }
    this.done = true;
    this.buffer = [];
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
