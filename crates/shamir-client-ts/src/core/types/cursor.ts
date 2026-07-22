/**
 * Cursor wire types (FG-5a) — type-only mirror of
 * `crates/shamir-query-types/src/wire/db_message.rs`'s
 * `CreateCursor` / `FetchNext` / `CancelCursor` / `CursorPage` /
 * `CursorClosed` variants and `crates/shamir-query-types/src/wire/cursor_id.rs`.
 *
 * These are top-level `DbRequest`/`DbResponse` shapes (the same tier as
 * `Ping`/`TxBegin`), NOT batch entries — there is no `BatchOp` for them.
 * See `docs/guide-docs/client-server-protocol-spec/CURSORS.md` for the full
 * wire contract.
 *
 * Pure type declarations; the constructor functions that produce these
 * shapes live in `../../builders/cursor.ts`.
 *
 * PLATFORM-AGNOSTIC.
 */

import type { ReadQuery } from './query.js';
import type { QueryResult } from './batch.js';

/**
 * Opaque server-assigned cursor handle. On the wire this is a bare `u64`
 * (`#[serde(transparent)]` on the Rust side), so it round-trips as a plain
 * number/bigint — never a wrapped object.
 */
export type CursorId = number | bigint;

/**
 * `DbRequest::CreateCursor` (`op: "create_cursor"`). Opens a server-side
 * cursor over `query` and returns its first page as a `CursorPage` response.
 */
export interface CreateCursorRequest {
  op: 'create_cursor';
  query_version?: number;
  db: string;
  query: ReadQuery;
  page_size: number;
}

/**
 * `DbRequest::FetchNext` (`op: "fetch_next"`). Fetches the next page from an
 * already-open cursor. `page_size` may differ per call.
 */
export interface FetchNextRequest {
  op: 'fetch_next';
  cursor_id: CursorId;
  page_size: number;
}

/**
 * `DbRequest::CancelCursor` (`op: "cancel_cursor"`). Idempotent: canceling
 * an unknown or already-closed cursor is not an error — the server replies
 * `CursorClosed` either way.
 */
export interface CancelCursorRequest {
  op: 'cancel_cursor';
  cursor_id: CursorId;
}

/**
 * `DbResponse::CursorPage` (`kind: "cursor_page"`). Reply to both
 * `CreateCursor` (first page) and `FetchNext` (subsequent pages).
 */
export interface CursorPageResponse {
  kind: 'cursor_page';
  cursor_id: CursorId;
  page: QueryResult;
  has_more: boolean;
}

/**
 * `DbResponse::CursorClosed` (`kind: "cursor_closed"`). Reply to
 * `CancelCursor` (also the idempotent reply when already closed/unknown).
 */
export interface CursorClosedResponse {
  kind: 'cursor_closed';
  cursor_id: CursorId;
}
