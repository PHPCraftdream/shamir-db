//! Typed operator-cancellation operation builders: a request, an interactive
//! transaction and another session's cursor.
//!
//! Each builder produces the sole operation of a standalone nontransactional
//! batch.
#![warn(missing_docs)]

use shamir_query_types::admin::activity::RequestExecId;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::wire::CursorId;

/// Build the sole operation for a standalone request-cancellation batch.
pub fn cancel_request(id: RequestExecId) -> BatchOp {
    BatchOp::CancelRequest(id)
}

/// Build the sole operation for a standalone transaction-cancellation batch.
///
/// Cancels an OPEN interactive transaction of another session (or another
/// transaction of the caller's own user) by the handle the activity view
/// shows. The server refuses a transaction that is already committing
/// (`already_committing`) and answers `already_finished` for an unknown
/// handle. Must be the sole operation of a standalone nontransactional
/// batch.
pub fn cancel_tx(tx_handle: u64) -> BatchOp {
    BatchOp::CancelTx(tx_handle)
}

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
