//! Transactional (MVCC) layer for ShamirDB.
//!
//! This crate hosts the engine-managed transactional machinery —
//! version codec, `RepoTxGate`, `TxContext`, `MvccStore`, `StagingStore`,
//! `IndexWriteOp`, `LayeredInterner`, `GcWorker`. The full design is
//! laid out in `docs/pre-transactional/` and `docs/roadmap/TRANSACTIONS.md`.
//!
//! ## Status
//!
//! **Stage 2 (in progress).** Landed primitives:
//!
//! - [`version_codec`] — `encode_version_key` / `decode_version_key` for
//!   `<key>::<version_be>` physical key layout.
//! - [`types`] — [`TxId`], [`IsolationLevel`] basic types.
//! - [`staging_store`] — [`StagingStore`] in-memory write buffer per tx.
//! - [`index_write_op`] — [`IndexWriteOp`] pure-data index mutation enum.
//! - [`repo_tx_gate`] — [`RepoTxGate`] per-repo commit serialisation + snapshots.
//! - [`tx_context`] — [`TxContext`] per-transaction state bundle.
//! - [`layered_interner`] — [`LayeredInterner`] two-mode interner wrapper +
//!   [`commit_interner_overlay`] merge (Stage 2.3).
//! - [`repo_wal_manager`] — [`RepoWalManager`] repo-level WAL for transactional
//!   writes, one `WalEntryV2` per tx/batch (Stage 2.4).
//!
//! **Stage 3.1 (in progress).** MvccStore landed:
//!
//! - [`mvcc_store`] — [`MvccStore`] versioned KV layer over main + history
//!   stores, zero-overhead when no snapshots are active.
//!
//! Stage 2 is now **complete** (RepoTxGate + TxContext + LayeredInterner +
//! RepoWalManager). Upcoming stages (see `docs/pre-transactional/`):
//! - Stage 6: `GcWorker`, `TxReaper`

#[cfg(test)]
mod tests;

pub mod cell_reservation_guard;
pub mod changefeed;
pub mod completion_tracker;
pub mod id_remap;
pub mod index_write_op;
pub mod layered_interner;
pub mod metrics;
pub mod mvcc_store;
pub mod pending_commit;
pub mod predicate_set;
pub mod repo_tx_gate;
pub mod repo_wal_manager;
pub mod staging_store;
pub mod tx_context;
pub mod types;
pub mod version_codec;
pub mod version_guard;
pub mod version_provider;
pub mod versioned_overlay;

pub use cell_reservation_guard::CellReservationGuard;
pub use changefeed::{
    nontx_event, project_event, version_key, ChangeOp, ChangelogEvent, ChangelogStore, JournalRead,
    RecordChange, RepoChangefeed, BROADCAST_CAPACITY, JOURNAL_CHANNEL_CAPACITY,
};
pub use completion_tracker::{CompletionTracker, State as CompletionState};
pub use id_remap::{remap_inner_value_bytes, remap_value};
pub use index_write_op::IndexWriteOp;
pub use layered_interner::{
    commit_interner_overlay, LayeredInterner, OverlayCommitResult, OVERLAY_ID_BASE,
};
pub use metrics::{TxMetrics, TxMetricsSnapshot};
pub use mvcc_store::{KeyLock, LockMode, MvccStore, Retention, VersionEntry};
pub use pending_commit::PendingCommit;
pub use predicate_set::{
    key_in_interval, PredicateDep, PredicateSet, SORTED_PREFIX_LEN, SORTED_TAG,
};
pub use repo_tx_gate::{
    build_footprint_from_tx, record_conflicts, CommitWriteRecord, RepoTxGate, SnapshotGuard,
    TableWriteFootprint,
};
pub use repo_wal_manager::RepoWalManager;
pub use staging_store::StagingStore;
pub use tx_context::{CommitVisibility, TxContext, UniqueGuard};
pub use types::{IsolationLevel, TxId};
pub use version_codec::{decode_version_key, encode_version_key};
pub use version_guard::VersionGuard;
pub use version_provider::VersionProvider;
pub use versioned_overlay::VersionedOverlay;
