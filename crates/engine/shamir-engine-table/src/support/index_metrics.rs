//! Index build/rebuild metrics seam for `TableManager`.
//!
//! Layering: `shamir-engine-table` cannot reach the instance-layer
//! `EngineMetrics` bundle (metrics flow downward only), so the create paths
//! observe through this narrow trait. `shamir-engine` implements the trait
//! for `EngineMetrics` and wires it via
//! [`TableManager::set_index_build_observer`]; the observer slot is shared
//! through `TableManager` clones (one `Arc<OnceLock<_>>` per manager), so a
//! late install on any clone covers all of them.
//!
//! Build vs rebuild mapping (observed call sites, one observe per COMPLETED
//! build; failed creates leave no live index and are NOT observed):
//! - build: `create_index_admitted` (regular, incl. its online Phase B/C/D
//!   path and the rename re-create under a NEW name), `create_unique_index_body`
//!   (unique family, kind regular), `create_sorted_index_with_include_admitted`
//!   (sorted), the `create_index_v2_admitted` non-btree FTS branch (fts).
//! - rebuild: `doctor::repair_admitted` re-invoking the create primitives for
//!   existing definitions (regular, unique, sorted), and the identity-v2
//!   migration's posting rebuild (`identity_migration.rs`). `repair()` does
//!   not rebuild index2 (fts) backends and the open-time Building-state
//!   re-backfill is recovery, not a user-visible rebuild — neither is
//!   observed (documented gap).
//!
//! Zero hot-path cost: the observer slot is read once per completed build.

use std::sync::Arc;
use std::time::Instant;

/// Index family of a build/rebuild observation. Mirrors the observable
/// families of the entry gauges: regular/unique hash indexes report as
/// `Regular`, the sorted (b-tree-by-value) family as `Sorted`, and index2
/// full-text backends as `Fts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMetricKind {
    Regular,
    Sorted,
    Fts,
}

impl IndexMetricKind {
    /// All kinds, ordered by [`IndexMetricKind::index`].
    pub const ALL: [IndexMetricKind; 3] = [
        IndexMetricKind::Regular,
        IndexMetricKind::Sorted,
        IndexMetricKind::Fts,
    ];

    /// Prometheus `kind` label value.
    pub const fn label(&self) -> &'static str {
        match self {
            IndexMetricKind::Regular => "regular",
            IndexMetricKind::Sorted => "sorted",
            IndexMetricKind::Fts => "fts",
        }
    }

    /// Stable slot index into the fixed-size histogram arrays.
    pub const fn index(&self) -> usize {
        match self {
            IndexMetricKind::Regular => 0,
            IndexMetricKind::Sorted => 1,
            IndexMetricKind::Fts => 2,
        }
    }
}

/// Downward metrics sink implemented by the instance-layer `EngineMetrics`.
pub trait IndexBuildObserver: Send + Sync {
    /// Observe one completed first-time index build.
    fn observe_index_build(&self, kind: IndexMetricKind, elapsed_ns: u64);

    /// Observe one completed rebuild (doctor/repair or identity migration).
    fn observe_index_rebuild(&self, kind: IndexMetricKind, elapsed_ns: u64);
}

/// Observe a finished create only on its success path; `started` must be the
/// `Instant` captured just before the create call.
pub(crate) fn observe_build(
    observer: Option<&Arc<dyn IndexBuildObserver>>,
    kind: IndexMetricKind,
    rebuild: bool,
    started: Instant,
) {
    let Some(observer) = observer else {
        return;
    };
    let elapsed_ns = started.elapsed().as_nanos() as u64;
    if rebuild {
        observer.observe_index_rebuild(kind, elapsed_ns);
    } else {
        observer.observe_index_build(kind, elapsed_ns);
    }
}
