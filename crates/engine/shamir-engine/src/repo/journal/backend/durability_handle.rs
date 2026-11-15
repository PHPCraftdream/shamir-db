//! `DurabilityHandle`: the durability bundle of a repository (ENGINE-LAYERING R2b).
//!
//! Plain struct of shared `Arc` cells moved out of `RepoInstance`; the shell
//! (instance layer) constructs it and clones it shallowly. Lazy `OnceCell`
//! cells keep their init where it was — the handle only holds the same cells.

pub(crate) mod changefeed;
pub(crate) mod retention_gate;

pub(crate) use changefeed::ChangefeedHandle;
use retention_gate::RetentionGate;

use crate::repo::changelog_store::write_domain::RepoWriteDomain;
use crate::repo::durability_policy::{
    DurabilityMetrics, DurabilityMetricsSnapshot, DurabilityPolicy,
};
use crate::repo::group_commit::GroupCommit;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Bundle of the durability-related cells of a repository (design §6.2).
#[derive(Clone)]
pub(crate) struct DurabilityHandle {
    /// Lazy-initialized RepoWalManager. Created on first call to `repo_wal()`.
    pub(crate) repo_wal: Arc<OnceCell<Arc<shamir_tx::RepoWalManager>>>,
    /// Shared initialized WAL coordinator for durable replication pulls.
    pub(crate) wal_group: Arc<OnceCell<Arc<shamir_wal::WalGroupCommit>>>,
    /// Group-commit coordinator for `synced` durability flushes.
    /// Shared across all clones so concurrent synced commits on this
    /// repo batch their flush+fsync into a single I/O round.
    pub(crate) group_commit: Arc<GroupCommit>,
    /// Lazily-initialised per-repo changefeed (Phase 3b): live broadcast +
    /// durable journal writer over the `"__changelog__"` store. Created on
    /// first call to the shell's `changefeed()`. Shared across clones.
    pub(crate) changefeed: Arc<OnceCell<ChangefeedHandle>>,
    /// Minimum unpublished-to-journal version after init failure; zero = none.
    pub(crate) pending_changefeed_gap: Arc<AtomicU64>,
    /// `backing_dir()` at construction. `Some(dir)` selects a file-backed WAL;
    /// `None` selects the in-memory WAL used by ephemeral backends.
    pub(crate) wal_dir: Option<std::path::PathBuf>,
    /// Shared policy installed before recovery and observed by every clone.
    pub(crate) durability: Arc<arc_swap::ArcSwap<DurabilityPolicy>>,
    pub(crate) durability_metrics: Arc<DurabilityMetrics>,
    /// PR2 S5c-X — lock-free WAL retention gate (refuse flag + counters);
    /// shared across clones like every other field.
    pub(crate) wal_retention_gate: Arc<RetentionGate>,
    /// Audit §1.7: serializes `persist_markers`' disk write of the recovery
    /// marker. `gate.last_committed()` is already a monotonic in-memory
    /// max, but two concurrent Phase 6.5 committers writing to the
    /// info_store with no ordering between their `Store::set` calls could
    /// still land the SMALLER value last on disk (a benign-looking write
    /// race, not a read-staleness bug). This mutex serializes the
    /// read-current/write-if-greater sequence in `persist_markers` so the
    /// on-disk marker is genuinely monotonic, not just the in-memory value
    /// passed to it. Shared across clones (one mutex per underlying repo,
    /// not per handle).
    pub(crate) marker_write_mutex: Arc<tokio::sync::Mutex<()>>,
    /// S5c-X PR6b-2a: repository write-admission domain, constructed BEFORE
    /// any recovery/publication; every clone shares one instance.
    pub(crate) write_domain: Arc<RepoWriteDomain>,
}

impl DurabilityHandle {
    pub(crate) fn initialized_mem_wal(&self) -> bool {
        self.wal_group
            .get()
            .is_some_and(|group| group.is_mem_sink())
    }

    /// PR1 Chunk B, amendment B2: this repo's durability SHARED cell.
    /// Read with `.load()` (one atomic load — the commit seam does exactly
    /// one per commit).
    pub(crate) fn durability(&self) -> &Arc<arc_swap::ArcSwap<DurabilityPolicy>> {
        &self.durability
    }

    /// Scrape initialized WAL and repo-owned pull observations; never opens stores.
    #[must_use]
    pub(crate) fn durability_metrics_snapshot(&self) -> DurabilityMetricsSnapshot {
        let mut snapshot = self.durability_metrics.snapshot(
            &self.durability.load(),
            self.wal_group.get().map(Arc::as_ref),
        );
        let retention = self.wal_retention_metrics();
        snapshot.journal = [
            retention[0],
            retention[1],
            retention[2],
            retention[3],
            retention[4],
            self.journal_dropped_total(),
        ];
        snapshot
    }

    /// Snapshot read of the shared policy; pins the value against concurrent stores.
    #[must_use]
    pub(crate) fn durability_policy(&self) -> arc_swap::Guard<Arc<DurabilityPolicy>> {
        self.durability.load()
    }

    /// Backend capability: does this repo persist a WAL across reopen?
    /// Read on the cold publication path only (replication DDL enforcement).
    pub(crate) fn has_persistent_wal(&self) -> bool {
        self.wal_dir.is_some()
    }
}

/// Handle-backed core: effective WAL commit tier from the shared policy cell.
pub(crate) fn wal_durability_for(handle: &DurabilityHandle) -> shamir_wal::WalDurability {
    let policy = handle.durability().load();
    match policy.effective_wal_commit() {
        crate::repo::WalCommitTier::Synced => shamir_wal::WalDurability::Synced,
        crate::repo::WalCommitTier::Buffered => shamir_wal::WalDurability::Buffered,
    }
}
