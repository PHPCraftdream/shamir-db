//! Shape-statistics reset request builder (release-1 R4b step 4).
//!
//! The typed [`ResetShapeStatsReq`] produced by the query-types crate is the
//! whole request; this constructor only wraps it into the typed
//! [`BatchOp::ResetShapeStats`] arm so callers never hand-assemble wire
//! values.
#![warn(missing_docs)]

use shamir_query_types::admin::shape_stats::ResetShapeStatsReq;
use shamir_query_types::batch::BatchOp;

/// Build the sole operation of a standalone shape-statistics reset batch.
///
/// Wipes one repository's local shape registry, including the `(other)` fold
/// row. The reset destroys local telemetry only; it is audited as an admin
/// operation, never as data. Requires `ActivityView` with `Action::Manage`;
/// the server answers with `DbResponse::ShapeStatsReset`. SCRAM session only
/// (API-token requests are denied server-side).
pub fn reset_shape_stats(request: ResetShapeStatsReq) -> BatchOp {
    BatchOp::ResetShapeStats(request)
}
