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
pub mod descriptor;
pub mod expr;
pub mod functional_backend;
pub mod kind;
pub mod posting_layout;
pub mod registry;

pub use actor::IndexActor;
pub use expr::{ExprError, IndexExpr};
pub use backend::{FtsMode, IndexBackend, IndexError, IndexQuery, IndexResult};
pub use descriptor::IndexDescriptor;
pub use kind::{
    FunctionalConfig, IndexKind, TokenizerKind, VectorBackendRef, VectorConfig, VectorMetric,
};
pub use posting_layout::{build_posting_key, type_tag, PostingKeyRef};
pub use registry::IndexRegistry;

#[cfg(test)]
mod enum_sizes {
    use super::*;
    use std::mem::size_of;

    /// Hot-path enums must not grow without explicit review.
    /// If you legitimately need to bump a bound, update it here and
    /// document why in the commit message.
    #[test]
    fn enum_sizes_under_limits() {
        assert!(
            size_of::<IndexKind>() <= 80,
            "IndexKind: {}",
            size_of::<IndexKind>()
        );
        // IndexQuery is created once per request (not per record),
        // so a slightly larger size is acceptable. Range carries two
        // `Bound<Vec<u8>>` (~80 bytes); Vector carries `Vec<f32>`.
        assert!(
            size_of::<IndexQuery>() <= 128,
            "IndexQuery: {}",
            size_of::<IndexQuery>()
        );
        assert!(
            size_of::<IndexResult>() <= 64,
            "IndexResult: {}",
            size_of::<IndexResult>()
        );
    }
}
