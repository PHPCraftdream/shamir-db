//! Typed request-cancellation operation builder.

use shamir_query_types::admin::activity::RequestExecId;
use shamir_query_types::batch::BatchOp;

/// Build the sole operation for a standalone request-cancellation batch.
pub fn cancel_request(id: RequestExecId) -> BatchOp {
    BatchOp::CancelRequest(id)
}
