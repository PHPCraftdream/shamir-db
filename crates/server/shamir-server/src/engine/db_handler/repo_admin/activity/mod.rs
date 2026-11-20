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

mod cancel;
mod continuation;
mod requests;

// The paging implementation (`list.rs`) implements `list_activity` as an
// method on `ShamirDbHandler`; no `pub` module re-export is needed.
pub(in crate::engine::db_handler) mod list;

// The guard and entry are named directly only by registry tests.
pub(in crate::engine::db_handler) use requests::server_busy_response;
pub(in crate::engine::db_handler) use requests::ActivityRequests;

#[cfg(test)]
pub(in crate::engine::db_handler) use requests::{InFlightRequest, RequestEntry};

#[cfg(test)]
mod tests;
