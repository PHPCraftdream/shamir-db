/**
 * Cursor lifecycle request constructors (FG-5a) — `createCursor` /
 * `fetchNext` / `cancelCursor`.
 *
 * These build top-level `DbRequest` wire objects (the same tier as
 * `{ op: 'ping' }` / `{ op: 'tx_begin', ... }`, which `ShamirClient` in
 * `../client.ts` constructs inline) — NOT batch entries, since there is no
 * `BatchOp` for a cursor op. `CreateCursor` embeds a `ReadQuery` (accept a
 * `Query` builder instance or a raw wire object, same convenience `Batch.add`
 * already offers); `FetchNext`/`CancelCursor` only need an existing cursor's
 * opaque id, so they take no query at all.
 *
 * Kept out of `../client.ts` deliberately: wiring these into a live
 * connection (an ergonomic streaming `Db` method / async iterator) is FG-5d,
 * a separate follow-up. This module only produces the wire shapes.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ReadQuery } from '../types/query.js';
import type {
  CreateCursorRequest,
  FetchNextRequest,
  CancelCursorRequest,
  CursorId,
} from '../types/cursor.js';

/** Something that has a `.build()` method returning a `ReadQuery`. */
interface QueryBuildable {
  build(): ReadQuery;
}

/** Resolve a `Query` builder instance or a raw `ReadQuery` object. */
function resolveReadQuery(query: ReadQuery | QueryBuildable): ReadQuery {
  return typeof (query as Partial<QueryBuildable>).build === 'function'
    ? (query as QueryBuildable).build()
    : (query as ReadQuery);
}

/**
 * Build a `DbRequest::CreateCursor` — opens a server-side cursor over
 * `query` on database `db`, with `pageSize` bounding the first (and, by
 * default, every subsequent) page.
 *
 * `query` may be a `Query` builder instance (`.build()` is called
 * automatically) or a raw `ReadQuery` wire object.
 */
export function createCursor(
  db: string,
  query: ReadQuery | QueryBuildable,
  pageSize: number,
): CreateCursorRequest {
  return {
    op: 'create_cursor',
    db,
    query: resolveReadQuery(query),
    page_size: pageSize,
  };
}

/**
 * Build a `DbRequest::FetchNext` — fetch the next page from an already-open
 * cursor. `pageSize` may differ from the size used at `createCursor` time or
 * any prior `fetchNext` call.
 */
export function fetchNext(cursorId: CursorId, pageSize: number): FetchNextRequest {
  return {
    op: 'fetch_next',
    cursor_id: cursorId,
    page_size: pageSize,
  };
}

/**
 * Build a `DbRequest::CancelCursor` — explicitly close an open cursor.
 * Idempotent: canceling an unknown or already-closed cursor is not an error
 * on the wire (see `CURSORS.md`).
 */
export function cancelCursor(cursorId: CursorId): CancelCursorRequest {
  return {
    op: 'cancel_cursor',
    cursor_id: cursorId,
  };
}
