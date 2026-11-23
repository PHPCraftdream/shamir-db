//! Typed interactive-transaction cancellation operation builder.
#![warn(missing_docs)]

use shamir_query_types::batch::BatchOp;

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
