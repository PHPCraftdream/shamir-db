//! Read execution: candidate-set plumbing and the top-level read dispatch
//! (split pieces of the former `read_exec`).
//! This module is a manifest: declarations and re-exports only.

pub(crate) mod candidate_set;
pub(crate) mod dispatch;
