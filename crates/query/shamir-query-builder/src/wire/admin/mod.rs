//! Narrow builders for top-level administration requests.
//!
//! This is a deliberate exception to the crate's no-`DbRequest`/no-hand-
//! assembled-wire rule: without these helpers callers must hand-assemble
//! request literals, contrary to the query-builder-only rule.
//!
//! The former `token_admin` module is grouped under this `admin` umbrella
//! unchanged (R4b step 1, activity view); `wire` keeps the old
//! `token_admin` path alive as a re-export of this submodule.

#![warn(missing_docs)]

mod cancel;
mod list_activity;
mod operator_state;
mod shape_stats;

pub mod token_admin;

pub use cancel::{cancel_cursor_admin, cancel_request, cancel_tx};
pub use list_activity::list_activity;
pub use operator_state::{get_record, status};
pub use shape_stats::{list_shape_stats, reset_shape_stats};
