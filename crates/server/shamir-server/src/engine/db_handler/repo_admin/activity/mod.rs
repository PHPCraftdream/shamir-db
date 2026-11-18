//! `activity/` — R4b-1 in-flight request table and the `ListActivity`
//! serving path for the operational-views activity view.
//!
//! - [`requests`] — the in-flight table (`scc::HashIndex`: lock-free
//!   readers; writers take brief internal bucket locks), the RAII
//!   registration guard (which also owns the per-session in-flight
//!   request slot), the fixed-text `server_busy` rejection, the gauge
//!   mirror and the state word.
//! - [`list`] — the `ListActivity` paging implementation, as inherent
//!   methods on `ShamirDbHandler`.

mod continuation;
mod requests;

// The paging implementation (`list.rs`) implements `list_activity` as an
// method on `ShamirDbHandler`; no `pub` module re-export is needed.
pub(in crate::engine::db_handler) mod list;

// Production code reaches `ActivityRequests` (handler field type) only;
// `RequestEntry` is constructed inside `requests.rs`
// (`try_track_activity_request`'s constructor path) and otherwise only named
// by private tests. The RAII guard's readable accessors serve production,
// so the test-facing imports (`InFlightRequest` type import, the raw
// state encodings and `RequestEntry`) are test-gated.
pub(in crate::engine::db_handler) use requests::ActivityRequests;
// Fixed-text `server_busy` rejection for saturated per-session request
// slots (execute, TxBegin, TxExecute — R4b-1 follow-up).
pub(in crate::engine::db_handler) use requests::server_busy_response;

#[cfg(test)]
pub(in crate::engine::db_handler) use requests::{
    InFlightRequest, RequestEntry, REQUEST_STATE_DONE, REQUEST_STATE_RUNNING,
};

#[cfg(test)]
mod tests;
