//! Read execution: candidate-set plumbing and the top-level read dispatch
//! (split pieces of the former `read_exec`).
//! This module is a manifest: declarations and re-exports only.

pub(in crate::table::read) mod candidate_set;
pub(in crate::table::read) mod dispatch;
