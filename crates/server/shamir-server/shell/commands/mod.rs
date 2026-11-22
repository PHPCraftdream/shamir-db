//! Shell command modules.
//!
//! PR1 carries the catalogue commands (`catalogue.rs`); PR3 adds the state
//! and narrow data view commands (`state.rs`); later PRs add the remaining
//! command files named by design §9 (access, activity, maintenance, jobs).
//! Declarations and re-exports ONLY in this file.

pub(in crate::shell) mod catalogue;
pub(in crate::shell) mod state;

pub(in crate::shell) use catalogue::Catalogue;
pub(in crate::shell) use state::StateCommand;
