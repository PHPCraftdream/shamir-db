//! Read-path module tree for the table engine: planning, execution dispatch,
//! pagination/vector dispatch slices and vector planning helpers.
//! This module is a manifest: declarations and re-exports only.

pub(crate) mod dispatch;
pub(crate) mod execution;
pub(crate) mod planning;
pub(crate) mod vector;
