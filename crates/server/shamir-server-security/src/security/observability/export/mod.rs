//! Per-area exporters of the metrics design (storage, functions,
//! replication). `mod.rs` re-exports only.

pub(crate) mod engine;
pub(crate) mod functions;
pub(crate) mod index_interner;
pub mod replication;
mod storage;
pub(crate) mod transactions;

pub(crate) use engine::{record_engine_metrics, register_engine_metrics};
pub(crate) use index_interner::{register_index_interner_metrics, sample_index_interner_metrics};
pub(crate) use storage::{record_storage_metrics, register_storage_metrics};
pub(crate) use transactions::{record_transactions_metrics, register_transactions_metrics};
