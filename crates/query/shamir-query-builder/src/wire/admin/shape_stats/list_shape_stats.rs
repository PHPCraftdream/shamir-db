//! Shape-statistics listing request builder (release-1 R4b step 4).
//!
//! The typed [`ListShapeStatsReq`] produced by the query-types crate is the
//! whole request; this constructor only wraps it into the typed
//! [`BatchOp::ListShapeStats`] arm so callers never hand-assemble wire values.
#![warn(missing_docs)]

use shamir_query_types::admin::shape_stats::ListShapeStatsReq;
use shamir_query_types::batch::BatchOp;

/// Build a read-only per-shape statistics listing operation.
///
/// Read-only by design: the listing renders canonical shape hashes and
/// counters only, and the server requires the `ActivityView` resource with
/// `Action::Manage`. Sent as the sole operation of a standalone
/// nontransactional batch; the server answers with
/// `DbResponse::ShapeStatsPage`. SCRAM session only (API-token requests are
/// denied server-side).
pub fn list_shape_stats(request: ListShapeStatsReq) -> BatchOp {
    BatchOp::ListShapeStats(request)
}
