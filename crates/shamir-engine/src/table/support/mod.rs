//! Shared support subtree for the table engine: model, storage, guards and
//! test seams. This module is a manifest: declarations and re-exports only.

pub mod guards;
pub mod model;
pub mod storage;
pub(super) mod test_seams;
