//! New index system foundation (Phase 0).
//!
//! Lives alongside `index/` (the existing IndexManager) without
//! touching it — incremental approach. Once skeletons are validated
//! and FTS / Functional / Vector backends land on top, the old
//! `index/` module is rewritten in terms of `index2::IndexBackend`.
//!
//! Architectural invariants (must hold across all impls):
//! - **Lock-free**: `scc::HashMap` (CAS) for registries,
//!   `arc_swap::ArcSwap` for RCU-style snapshot reads, `AtomicU*`
//!   counters. NO `std::sync::Mutex` / `RwLock` / `parking_lot`.
//! - **Async**: `IndexBackend` is `#[async_trait]`. Stateful backends
//!   (FTS / Vector) submit writes via `tokio::sync::mpsc` to a
//!   single applier task — readers never block.
//! - **Zero-copy**: `PostingKeyRef<'a>` decodes from raw bytes
//!   without allocations.
//! - **O(1)** where possible: hashmap dispatch, atomic counters.

pub mod actor;
pub mod backend;
pub mod bm25;
pub mod build_backend;
pub mod descriptor;
pub mod expr;
pub mod fts_backend;
pub mod fts_ranked_backend;
pub mod functional_backend;
pub mod kind;
pub mod legacy;
pub mod meta_envelope;
pub mod persistence;

pub mod posting_layout;
pub mod registry;
pub mod tokenizer;
pub mod vector;
pub mod write_ops;

pub use actor::IndexActor;
pub use backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
pub use build_backend::build_index2_backend;
pub use descriptor::IndexDescriptor;
pub use expr::{ExprError, IndexExpr};
pub use kind::{
    FunctionalConfig, IndexKind, TokenizerKind, VectorBackendRef, VectorConfig, VectorMetric,
};
pub use meta_envelope::{MetaEnvelope, MetaError, ENVELOPE_MAGIC, ENVELOPE_VERSION};
pub use persistence::{
    legacy_indexes_need_rebuild, load_legacy_index_version, save_legacy_index_version,
    LEGACY_INDEX_FORMAT_VERSION,
};
pub use posting_layout::{build_posting_key, type_tag, PostingKeyRef};
pub use registry::IndexRegistry;
pub use write_ops::{apply_index_ops, IndexWriteOp};

#[cfg(test)]
mod tests;
