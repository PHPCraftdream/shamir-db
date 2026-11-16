//! Execution-phase slices of the former `read_exec` read pipeline.
//! This module is a manifest: declarations and re-exports only.

pub(crate) mod collect;
pub(crate) mod explain;
pub(crate) mod paged;
pub(crate) mod scan;
pub(crate) mod vector;
