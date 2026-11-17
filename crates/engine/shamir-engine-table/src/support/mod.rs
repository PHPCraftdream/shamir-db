//! Shared support subtree for the table engine: model, storage, guards and
//! test seams. This module is a manifest: declarations and re-exports only.

pub mod guards;
pub mod index_metrics;
pub mod model;
pub mod storage;
pub mod table_resolver;
pub(super) mod test_seams;
pub mod validator_bound;
