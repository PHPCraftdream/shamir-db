use crate::repo::fk_reverse_cache::FkReverseCache;
use crate::repo::journal::backend::durability_handle::retention_gate::RetentionGate;
use crate::repo::repo_types::BoxRepo;
use crate::repo::version_provider::register_token;
pub use crate::repo::version_provider::repo_token;
use crate::repo::DurabilityHandle;
use crate::table::{TableCatalog, TableConfig, TableManager};
use crate::tx::apply_replicated::follower_durability::receipt::{
    ApplyOutcome, ReceiptApplyJob, ReceiptLease,
};
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;
use crate::tx::cold_reconcile::lifecycle::ForegroundLifecycle;
use crate::tx::CommitTailBarrier;
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;
use shamir_storage::types::Repo as _;
use shamir_types::types::common::{new_dash_map_wc, TDashMap, THasher};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::OnceCell;

pub(in crate::repo) mod catalog;
mod lifecycle;
pub(crate) mod table_store;
mod transactions;

pub(crate) use crate::repo::journal::backend::durability_handle::ChangefeedHandle;

/// Manages a single repository and its tables.
///
/// Trusted native-embedding handle; raw operations do not authorize an actor.
/// Use [`crate::query::batch::Authorized`] with the `shamir-db` facade's
/// live ACL gate for authorization-enforced batch execution.
pub struct RepoInstance {
    name: String,
    commit_visibility_wait: VisibilityWait,
    pub(in crate::repo) repo: BoxRepo,
    /// Plain Arc-shared catalogue of this repo's tables; constructed below,
    /// cloned shallowly.
    catalog: TableCatalog,
    /// Lazy-initialized RepoTxGate. Created on first call to `tx_gate()`.
    tx_gate: Arc<OnceCell<Arc<shamir_tx::RepoTxGate>>>,
    /// In-doubt rename entries indexed by both involved names.
    rename_fence: Arc<
        scc::HashMap<
            String,
            super::repo_instance::catalog::rename_in_doubt::RenameFenceEntry,
            THasher,
        >,
    >,
    /// O(1) count of owned source fence entries.
    rename_fence_len: Arc<AtomicUsize>,
    /// Atomic tx telemetry counters.
    tx_metrics: Arc<shamir_tx::TxMetrics>,
    /// Counts owned inline and detached commits for explicit shutdown waiting.
    commit_tail: Arc<CommitTailBarrier>,
    #[cfg(test)]
    pub(crate) journal_publish_hook: Arc<
        arc_swap::ArcSwapOption<crate::tx::tests::journal_publish_order_tests::JournalPublishHook>,
    >,
    /// D2 P1d-2b: lazily-spawned background drainer (history-write off the
    /// ack-path). Created + spawned once on first [`drainer`](Self::drainer)
    /// call; shared across foreground clones. The drain TASK holds a
    /// background clone of this repo (`live = None`) plus a `Weak<()>` of
    /// [`live`](Self::live), so it exits + releases when the last foreground
    /// clone drops — see [`Drainer::spawn`](crate::tx::drainer::Drainer::spawn).
    ///
    /// `std::sync::OnceLock` (not tokio's async `OnceCell`): the
    /// [`drainer`](Self::drainer) accessor is SYNC (called from the commit
    /// tail to `wake()`), and init does no `.await` — it only constructs
    /// the drainer + `tokio::spawn`s the loop. Single-init is guaranteed by
    /// `OnceLock::get_or_init`: the closure runs EXACTLY once even under
    /// concurrent callers (the loser waits for the winner's value and
    /// returns the same `Arc<Drainer>`), so there is no "lost race" — every
    /// caller observes the one-and-only drainer.
    drainer: Arc<std::sync::OnceLock<Arc<crate::tx::drainer::Drainer>>>,
    /// Fixed-size foreground lifecycle token; background clones omit it.
    pub(crate) worker_lifecycle:
        Option<Arc<ForegroundLifecycle<ReceiptApplyJob, DbResult<ApplyOutcome>>>>,
    pub(crate) receipt_worker: Arc<OnceCell<Arc<ReceiptLease>>>,
    /// D2 P1d-2b: repo liveness token. `Some(Arc<()>)` on every FOREGROUND
    /// clone (the strong count == number of live foreground clones); `None`
    /// on the single BACKGROUND clone the drain task owns
    /// ([`clone_for_background`](Self::clone_for_background)) so the task does
    /// NOT keep the repo alive. The drain loop holds a `Weak<()>` of this and
    /// exits when the strong count reaches zero.
    live: Option<Arc<()>>,
    /// F-28 Step 4 (#831): cached per-repo reverse-FK map (parent table name
    /// → its reverse-FK entries) plus the child→parents reverse-reverse
    /// index, replacing the O(tables) `collect_fk_refs()` scan
    /// `fk_restrict.rs`/`fk_actions.rs` used to repeat on EVERY
    /// delete/cascade-recursion-level. Lazily populated (cache-aside) and
    /// invalidated wholesale on any schema mutation that can change what
    /// that scan would discover — see [`FkReverseCache::invalidate`] and its
    /// callers ([`add_table`](Self::add_table), [`remove_table`](Self::remove_table),
    /// `ShamirDb::compile_table_schema`). Shared across clones (one cache
    /// per underlying repo, not per handle).
    fk_reverse_cache: Arc<FkReverseCache>,
    #[cfg(test)]
    pub(crate) table_init_hook:
        Arc<arc_swap::ArcSwapOption<crate::repo::tests::table_init_retry_tests::TableInitHook>>,
    /// Per-open cache for the completed-reseed lifecycle record (see `durability_repo_impl.rs`).
    replication_armed: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    clean_horizon_test_state: Arc<std::sync::OnceLock<Arc<std::sync::Mutex<test_seam::State>>>>,
    #[cfg(test)]
    background_drainer_disabled: Arc<std::sync::atomic::AtomicBool>,
    wal_segment_bytes: u64,
    /// Durability bundle (R2b): the durability-related Arc cells of this
    /// repo, owned by the repo layer; cloned shallowly like every field.
    pub(crate) durability_handle: DurabilityHandle,
}

impl Clone for RepoInstance {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            commit_visibility_wait: self.commit_visibility_wait,
            repo: self.repo.clone(),
            catalog: self.catalog.clone(),
            tx_gate: Arc::clone(&self.tx_gate),
            rename_fence: Arc::clone(&self.rename_fence),
            rename_fence_len: Arc::clone(&self.rename_fence_len),
            tx_metrics: Arc::clone(&self.tx_metrics),
            commit_tail: Arc::clone(&self.commit_tail),
            #[cfg(test)]
            journal_publish_hook: Arc::clone(&self.journal_publish_hook),
            drainer: Arc::clone(&self.drainer),
            worker_lifecycle: self.worker_lifecycle.clone(),
            receipt_worker: Arc::clone(&self.receipt_worker),
            // Foreground clone: carries a strong liveness ref. If `self` is
            // itself the background clone (`None`) this stays `None` — a clone
            // of the background handle must not resurrect liveness.
            live: self.live.clone(),
            fk_reverse_cache: Arc::clone(&self.fk_reverse_cache),
            #[cfg(test)]
            table_init_hook: Arc::clone(&self.table_init_hook),
            replication_armed: Arc::clone(&self.replication_armed),
            #[cfg(test)]
            clean_horizon_test_state: Arc::clone(&self.clean_horizon_test_state),
            #[cfg(test)]
            background_drainer_disabled: Arc::clone(&self.background_drainer_disabled),
            wal_segment_bytes: self.wal_segment_bytes,
            durability_handle: self.durability_handle.clone(),
        }
    }
}

impl RepoInstance {
    /// Construct an unregistered low-level handle. Reused backing stores MUST
    /// run recover_v2_inflight before any caller serves reads/writes. Registry
    /// entry points enforce that ordering; this synchronous fixture constructor
    /// cannot perform asynchronous recovery.
    pub fn new(name: String, repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        Self::from_box_repo(name, repo, configs)
    }

    fn from_box_repo(name: String, repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        // In-memory / test path: no disk backing → KV-marker WAL.
        Self::from_box_repo_with_wal_dir(name, repo, configs, None)
    }

    pub(in crate::repo::instance::repo_instance) fn from_box_repo_with_wal_dir(
        name: String,
        repo: BoxRepo,
        configs: Vec<TableConfig>,
        wal_dir: Option<std::path::PathBuf>,
    ) -> Self {
        let initial_cap = configs.len().max(16);
        let configs_map: TDashMap<String, TableConfig> = new_dash_map_wc(initial_cap);
        let token_names: scc::HashMap<u64, String, THasher> =
            scc::HashMap::with_capacity_and_hasher(initial_cap, THasher::default());
        for cfg in configs {
            register_token(&token_names, &cfg.name);
            configs_map.insert(cfg.name.clone(), cfg);
        }

        let tables: TDashMap<String, Arc<OnceCell<TableManager>>> = new_dash_map_wc(100);

        let catalog = TableCatalog {
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
            token_names: Arc::new(token_names),
            per_table_mvcc: Arc::new(scc::HashMap::with_capacity_and_hasher(
                initial_cap,
                THasher::default(),
            )),
            repo_interner: Arc::new(OnceCell::new()),
        };
        let policy = crate::repo::durability_policy::DurabilityPolicy::default()
            .for_wal_backend(wal_dir.is_some());
        let durability_handle = DurabilityHandle {
            repo_wal: Arc::new(OnceCell::new()),
            wal_group: Arc::new(OnceCell::new()),
            group_commit: Arc::new(crate::repo::group_commit::GroupCommit::new()),
            changefeed: Arc::new(OnceCell::new()),
            pending_changefeed_gap: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            wal_dir,
            durability: Arc::new(arc_swap::ArcSwap::from_pointee(policy)),
            durability_metrics: Arc::new(
                crate::repo::durability_policy::DurabilityMetrics::default(),
            ),
            wal_retention_gate: Arc::new(RetentionGate::default()),
            marker_write_mutex: Arc::new(tokio::sync::Mutex::new(())),
            write_domain: crate::repo::changelog_store::write_domain::RepoWriteDomain::new(
                name.clone(),
            ),
        };
        Self {
            name,
            commit_visibility_wait: VisibilityWait::Strict,
            repo,
            catalog,
            tx_gate: Arc::new(OnceCell::new()),
            rename_fence: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            rename_fence_len: Arc::new(AtomicUsize::new(0)),
            tx_metrics: Arc::new(shamir_tx::TxMetrics::new()),
            worker_lifecycle: Some(Arc::new(ForegroundLifecycle::new())),
            receipt_worker: Arc::new(OnceCell::new()),
            commit_tail: Arc::new(CommitTailBarrier::default()),
            #[cfg(test)]
            journal_publish_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            drainer: Arc::new(std::sync::OnceLock::new()),
            live: Some(Arc::new(())),
            fk_reverse_cache: Arc::new(FkReverseCache::new()),
            #[cfg(test)]
            table_init_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            replication_armed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            clean_horizon_test_state: Arc::new(std::sync::OnceLock::new()),
            #[cfg(test)]
            background_drainer_disabled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wal_segment_bytes: crate::repo::durability_policy::effective_wal_segment_bytes(),
            durability_handle,
        }
    }
}

/// tx-facing seam (`tx/recovery/clean_horizon/checkpoint.rs` imports
/// `crate::repo::durability_policy::checkpoint_allocation_floor`). Resolves
/// the journal store and delegates to the repo-layer core.
pub(crate) async fn checkpoint_allocation_floor(repo: &RepoInstance) -> DbResult<u64> {
    let store = repo.repo.store_get("__changelog__").await?;
    crate::repo::durability_policy::checkpoint_allocation_floor_for(store).await
}
