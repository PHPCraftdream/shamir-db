//! Typed operator result-cursor cancellation operation builder.
#![warn(missing_docs)]

use shamir_query_types::batch::BatchOp;
use shamir_query_types::wire::CursorId;

/// Build the sole operation for a standalone operator cursor-cancellation
/// batch.
///
/// Operator cancellation of a result cursor that belongs to another session
/// (or another session of the caller's own user). It does NOT change the
/// semantics of the existing session-scoped `DbRequest::CancelCursor` wire
/// request. Outcomes are `cancelled` / `already_finished` only. Must be the
/// sole operation of a standalone nontransactional batch.
pub fn cancel_cursor_admin(cursor_id: CursorId) -> BatchOp {
    BatchOp::CancelCursorAdmin(cursor_id)
}
