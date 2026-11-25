//! Admin DTOs for the activity view (R4b step 1): the typed
//! `BatchOp::ListActivity` request and the `ActivityPage` response rows.
//!
//! Pure wire/serde types; classification (read-only, server-owned state
//! introspection, follower-safe) lives in `BatchOp`, authorization and
//! rendering in the server's admin handler (§5.1–§5.2).

mod activity_page;
mod activity_row;
mod cancel_outcome;
mod list_activity_req;
mod op_name;

pub use activity_page::ActivityPage;
pub use activity_row::{ActivityRequestState, ActivityRow, ActivityTxState, RequestExecId};
pub use cancel_outcome::CancelOutcome;
pub use list_activity_req::{ActivityKind, ActivityScope, ListActivityReq};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
