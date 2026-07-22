//! Cursor lifecycle requests (FG-5a) — `CreateCursor` / `FetchNext` /
//! `CancelCursor`.
//!
//! # Why this module exists despite the crate's "no `DbRequest`" rule
//!
//! [`crate`]'s module doc states the builder deliberately does not construct
//! top-level [`DbRequest`] envelope variants (`Ping`, `TxBegin`, …) — those
//! are a transport/session-lifecycle concern owned by the client SDKs.
//! Cursor lifecycle ops are a narrow, deliberate exception to that rule:
//!
//! - [`create_cursor`] takes a [`ReadQuery`] — the SAME query shape
//!   [`Batch::query`](crate::batch::Batch::query) already builds — and folds
//!   it into a [`DbRequest::CreateCursor`]. Without this helper, callers
//!   would have to hand-assemble the `DbRequest` struct literal themselves,
//!   which is exactly the "no raw wire assembly" rule
//!   ("Query construction — builder only", `CLAUDE.md`) forbids.
//! - [`fetch_next`] / [`cancel_cursor`] reference an EXISTING cursor by
//!   opaque id — there is no query to build, so they are not `Batch` entries
//!   (a `Batch` accumulates `BatchOp`s that ride inside `Execute`; cursor
//!   ops are sibling top-level `DbRequest`s, the same tier as `TxCommit`/
//!   `TxRollback`). They are exposed here as small free functions so callers
//!   never need to write `DbRequest::FetchNext { .. }` by hand either.
//!
//! See `docs/guide-docs/client-server-protocol-spec/CURSORS.md` for the full
//! wire contract (this module builds exactly those three request shapes,
//! nothing more).

use shamir_query_types::read::ReadQuery;
use shamir_query_types::wire::{CursorId, DbRequest, CURRENT_QUERY_LANG_VERSION};

/// Build a [`DbRequest::CreateCursor`] — opens a server-side cursor over
/// `query` on database `db`, with `page_size` bounding the first (and, by
/// default, every subsequent) page.
///
/// Uses [`CURRENT_QUERY_LANG_VERSION`]; use [`create_cursor_with_version`]
/// to pin an explicit version.
pub fn create_cursor(
    db: impl Into<String>,
    query: impl Into<ReadQuery>,
    page_size: u32,
) -> DbRequest {
    create_cursor_with_version(CURRENT_QUERY_LANG_VERSION, db, query, page_size)
}

/// Like [`create_cursor`] but with an explicit `query_version`.
pub fn create_cursor_with_version(
    query_version: u32,
    db: impl Into<String>,
    query: impl Into<ReadQuery>,
    page_size: u32,
) -> DbRequest {
    DbRequest::CreateCursor {
        query_version,
        db: db.into(),
        query: query.into(),
        page_size,
    }
}

/// Build a [`DbRequest::FetchNext`] — fetch the next page from an
/// already-open cursor. `page_size` may differ from the size used at
/// `CreateCursor` time or any prior `FetchNext` call.
pub fn fetch_next(cursor_id: impl Into<CursorId>, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id: cursor_id.into(),
        page_size,
    }
}

/// Build a [`DbRequest::CancelCursor`] — explicitly close an open cursor.
/// Idempotent: canceling an unknown or already-closed cursor is not an
/// error on the wire (see `CURSORS.md`).
pub fn cancel_cursor(cursor_id: impl Into<CursorId>) -> DbRequest {
    DbRequest::CancelCursor {
        cursor_id: cursor_id.into(),
    }
}

#[cfg(test)]
mod tests;
