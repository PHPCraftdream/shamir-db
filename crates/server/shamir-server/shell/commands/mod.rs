//! Shell command modules.
//!
//! PR1 carries the catalogue commands only (`catalogue.rs`); later PRs add
//! the remaining command files named by design §9 (state, access, activity,
//! maintenance, jobs). Declarations and re-exports ONLY in this file.

pub(in crate::shell) mod catalogue;

pub(in crate::shell) use catalogue::Catalogue;
