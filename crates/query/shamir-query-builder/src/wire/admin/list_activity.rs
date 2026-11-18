//! Activity-view listing request builder (release-1 R4b step 1).
//!
//! The typed [`ListActivityReq`] produced by the query-types crate is the
//! whole request; this constructor only wraps it into the typed
//! [`BatchOp::ListActivity`] arm so callers never hand-assemble wire values.
#![warn(missing_docs)]

use shamir_query_types::admin::activity::ListActivityReq;
use shamir_query_types::batch::BatchOp;

/// Build a read-only operator activity-view listing operation.
///
/// Read-only by design (operational-views §5.2): `scope: Own` is open to
/// any authenticated user, `scope: All` requires the `ActivityView`
/// resource with `Action::Manage`; the server authorizes. Sent as the sole
/// operation of a standalone nontransactional batch; the server answers
/// with `DbResponse::ActivityPage`.
pub fn list_activity(request: ListActivityReq) -> BatchOp {
    BatchOp::ListActivity(request)
}
