//! Read-path module tree for the table engine: planning, execution dispatch,
//! pagination/vector dispatch slices and vector planning helpers.
//! This module is a manifest: declarations and re-exports only.

pub(in crate::table) mod dispatch;
pub(in crate::table) mod execution;
pub(in crate::table) mod planning;
pub(in crate::table) mod vector;
