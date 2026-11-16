//! Per-area exporters of the metrics design (storage, functions,
//! replication). `mod.rs` re-exports only.

pub(crate) mod functions;
pub mod replication;
mod storage;

pub(crate) use storage::{record_storage_metrics, register_storage_metrics};
