//! Execution-phase slices of the former `read_exec` read pipeline.
//! This module is a manifest: declarations and re-exports only.

pub(in crate::table::read) mod collect;
pub(in crate::table::read) mod explain;
pub(in crate::table::read) mod paged;
pub(in crate::table::read) mod scan;
pub(in crate::table::read) mod vector;
