//! Transactional (MVCC) layer for ShamirDB.
//!
//! This crate hosts the engine-managed transactional machinery —
//! version codec, `RepoTxGate`, `TxContext`, `MvccStore`, `StagingStore`,
//! `IndexWriteOp`, `LayeredInterner`, `GcWorker`. The full design is
//! laid out in `docs/pre-transactional/` and `docs/roadmap/TRANSACTIONS.md`.
//!
//! ## Status
//!
//! **Foundation phase.** Only the lowest-level primitives are landed:
//!
//! - [`version_codec`] — `encode_version_key` / `decode_version_key` for
//!   `<key>::<version_be>` physical key layout.
//! - [`types`] — [`TxId`], [`IsolationLevel`], [`TxConflict`] basic types.
//!
//! Upcoming stages (see `docs/pre-transactional/`):
//! - Stage 1: `IndexWriteOp`, `StagingStore`
//! - Stage 2: `RepoTxGate`, `TxContext`, `LayeredInterner`
//! - Stage 3: `MvccStore`
//! - Stage 6: `GcWorker`, `TxReaper`

pub mod index_write_op;
pub mod repo_tx_gate;
pub mod staging_store;
pub mod tx_context;
pub mod types;
pub mod version_codec;

pub use index_write_op::IndexWriteOp;
pub use repo_tx_gate::{RepoTxGate, SnapshotGuard};
pub use staging_store::StagingStore;
pub use tx_context::TxContext;
pub use types::{IsolationLevel, TxConflict, TxError, TxId};
pub use version_codec::{decode_version_key, encode_version_key};
