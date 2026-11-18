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

mod list_activity;

pub mod token_admin;

pub use list_activity::list_activity;
