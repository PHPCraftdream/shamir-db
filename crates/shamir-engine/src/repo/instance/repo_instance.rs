use crate::repo::changelog_store::write_domain::RepoWriteDomain;
use crate::repo::durability_policy::{DurabilityMetrics, DurabilityPolicy};
use crate::repo::fk_reverse_cache::FkReverseCache;
use crate::repo::group_commit::GroupCommit;
use crate::repo::repo_types::BoxRepo;
use crate::repo::version_provider::register_token;
pub use crate::repo::version_provider::repo_token;
use crate::table::interner_manager::InternerManager;
use crate::table::{TableConfig, TableManager};
use crate::tx::apply_replicated::follower_durability::receipt::{
    ApplyOutcome, ReceiptApplyJob, ReceiptLease,
};
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;
use crate::tx::cold_reconcile::lifecycle::ForegroundLifecycle;
use crate::tx::CommitTailBarrier;
use shamir_query_types::batch::VisibilityWait;
use shamir_storage::error::DbResult;

use shamir_types::types::common::{new_dash_map_wc, TDashMap, THasher};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

pub(in crate::repo) mod catalog;
mod journal;
mod lifecycle;
mod transactions;

/// Fixed read-side budget, not a tunable: a stalled writer returns an older
/// safe journal prefix after at most one second of admission + confirmation.
/// Barriers share the bounded writer channel's slots with journal events.
const JOURNAL_CONFIRM_BUDGET: Duration = Duration::from_secs(1);

/// Manages a single repository and its tables
pub struct RepoInstance {
    name: String,
    commit_visibility_wait: VisibilityWait,
    pub(in crate::repo) repo: BoxRepo,
    configs: Arc<TDashMap<String, TableConfig>>,
    tables: Arc<TDashMap<String, Arc<OnceCell<TableManager>>>>,
    /// Lazy-initialized RepoTxGate. Created on first call to `tx_gate()`.
    tx_gate: Arc<OnceCell<Arc<shamir_tx::RepoTxGate>>>,
    /// Lazy-initialized RepoWalManager. Created on first call to `repo_wal()`.
    repo_wal: Arc<OnceCell<Arc<shamir_tx::RepoWalManager>>>,
    /// Shared initialized WAL coordinator for durable replication pulls.
    wal_group: Arc<OnceCell<Arc<shamir_wal::WalGroupCommit>>>,
    /// Per-table MvccStore map for SSI version provider. Populated
    /// on demand when `create_table_context` instantiates a
    /// TableManager — both share the same data_store reference.
    /// Key = `table_token_for(name)` (deterministic).
    per_table_mvcc: Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>>,
    /// Reverse index `table_token_for(name) → name`, maintained at table
    /// *registration* time (`from_box_repo` + `add_table`), independent of
    /// whether the table has been instantiated yet. Lets `table_by_token`
    /// resolve a token in O(1) instead of scanning every config and
    /// re-hashing its name under the per-repo `commit_lock` (III.1). The
    /// token is a pure function of the name, so this is just a
    /// pre-computed inverse of that function.
    token_names: Arc<scc::HashMap<u64, String, THasher>>,
    /// Atomic tx telemetry counters.
    tx_metrics: Arc<shamir_tx::TxMetrics>,
    /// Counts owned inline and detached commits for explicit shutdown waiting.
    commit_tail: Arc<CommitTailBarrier>,
    /// Group-commit coordinator for `synced` durability flushes.
    /// Shared across all clones so concurrent synced commits on this
    /// repo batch their flush+fsync into a single I/O round.
    group_commit: Arc<GroupCommit>,
    /// Lazily-initialised per-repo changefeed (Phase 3b): live broadcast +
    /// durable journal writer over the `"__changelog__"` store. Created on
    /// first call to [`changefeed`](Self::changefeed). Shared across clones.
    changefeed: Arc<OnceCell<ChangefeedHandle>>,
    /// Minimum unpublished-to-journal version after init failure; zero = none.
    pending_changefeed_gap: Arc<AtomicU64>,
    #[cfg(test)]
    pub(crate) journal_publish_hook: Arc<
        arc_swap::ArcSwapOption<crate::tx::tests::journal_publish_order_tests::JournalPublishHook>,
    >,
    /// `backing_dir()` at construction. `Some(dir)` selects a file-backed WAL;
    /// `None` selects the in-memory WAL used by ephemeral backends.
    wal_dir: Option<std::path::PathBuf>,
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
    /// Per-repo string interner (Stage I — moved from per-table to per-repo).
    /// Backed by the dedicated `"__interner__"` store. Built once on first
    /// access (lazy) and shared via [`Arc::clone`] so every
    /// [`TableManager`](crate::table::TableManager) in this repo shares the
    /// same live [`Interner`](shamir_types::core::interner::Interner) and
    /// id-namespace. This is what makes a field name resolve to ONE id across
    /// every table in the repo, and what lets V2 WAL recovery resolve a single
    /// interner instead of a per-table one.
    repo_interner: Arc<OnceCell<InternerManager>>,
    /// D2 P1d-2b: repo liveness token. `Some(Arc<()>)` on every FOREGROUND
    /// clone (the strong count == number of live foreground clones); `None`
    /// on the single BACKGROUND clone the drain task owns
    /// ([`clone_for_background`](Self::clone_for_background)) so the task does
    /// NOT keep the repo alive. The drain loop holds a `Weak<()>` of this and
    /// exits when the strong count reaches zero.
    live: Option<Arc<()>>,
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
    marker_write_mutex: Arc<tokio::sync::Mutex<()>>,
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
    /// Shared policy installed before recovery and observed by every clone.
    durability: Arc<arc_swap::ArcSwap<DurabilityPolicy>>,
    durability_metrics: Arc<DurabilityMetrics>,
    /// Per-open cache for the completed-reseed lifecycle record (see `durability_repo_impl.rs`).
    replication_armed: Arc<std::sync::atomic::AtomicBool>,
    /// PR2 S5c-X — lock-free WAL retention gate (refuse flag + counters);
    /// shared across clones like every other field.
    wal_retention_gate: Arc<crate::tx::clean_horizon::retention_gate::RetentionGate>,
    #[cfg(test)]
    clean_horizon_test_state: Arc<std::sync::OnceLock<Arc<std::sync::Mutex<test_seam::State>>>>,
    #[cfg(test)]
    background_drainer_disabled: Arc<std::sync::atomic::AtomicBool>,
    wal_segment_bytes: u64,
    /// S5c-X PR6b-2a: repository write-admission domain, constructed BEFORE
    /// any recovery/publication; every clone shares one instance.
    pub(in crate::repo) write_domain: Arc<RepoWriteDomain>,
}

/// Bundle of the per-repo changefeed and the store it journals into.
/// The store is retained so [`RepoInstance::read_changelog_from`] can
/// range-read the journal without re-resolving it.
#[derive(Clone)]
pub(crate) struct ChangefeedHandle {
    pub(crate) feed: Arc<shamir_tx::RepoChangefeed>,
    pub(crate) store: Arc<dyn shamir_tx::ChangelogStore>,
}

impl Clone for RepoInstance {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            commit_visibility_wait: self.commit_visibility_wait,
            repo: self.repo.clone(),
            configs: Arc::clone(&self.configs),
            tables: Arc::clone(&self.tables),
            tx_gate: Arc::clone(&self.tx_gate),
            repo_wal: Arc::clone(&self.repo_wal),
            per_table_mvcc: Arc::clone(&self.per_table_mvcc),
            token_names: Arc::clone(&self.token_names),
            tx_metrics: Arc::clone(&self.tx_metrics),
            commit_tail: Arc::clone(&self.commit_tail),
            group_commit: Arc::clone(&self.group_commit),
            changefeed: Arc::clone(&self.changefeed),
            pending_changefeed_gap: Arc::clone(&self.pending_changefeed_gap),
            #[cfg(test)]
            journal_publish_hook: Arc::clone(&self.journal_publish_hook),
            wal_dir: self.wal_dir.clone(),
            drainer: Arc::clone(&self.drainer),
            worker_lifecycle: self.worker_lifecycle.clone(),
            receipt_worker: Arc::clone(&self.receipt_worker),
            repo_interner: Arc::clone(&self.repo_interner),
            // Foreground clone: carries a strong liveness ref. If `self` is
            // itself the background clone (`None`) this stays `None` — a clone
            // of the background handle must not resurrect liveness.
            live: self.live.clone(),
            marker_write_mutex: Arc::clone(&self.marker_write_mutex),
            fk_reverse_cache: Arc::clone(&self.fk_reverse_cache),
            #[cfg(test)]
            table_init_hook: Arc::clone(&self.table_init_hook),
            wal_group: Arc::clone(&self.wal_group),
            durability: Arc::clone(&self.durability),
            durability_metrics: Arc::clone(&self.durability_metrics),
            replication_armed: Arc::clone(&self.replication_armed),
            wal_retention_gate: Arc::clone(&self.wal_retention_gate),
            #[cfg(test)]
            clean_horizon_test_state: Arc::clone(&self.clean_horizon_test_state),
            #[cfg(test)]
            background_drainer_disabled: Arc::clone(&self.background_drainer_disabled),
            wal_segment_bytes: self.wal_segment_bytes,
            write_domain: Arc::clone(&self.write_domain),
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

        let policy = DurabilityPolicy::default().for_wal_backend(wal_dir.is_some());
        let write_domain = RepoWriteDomain::new(name.clone());
        Self {
            name,
            commit_visibility_wait: VisibilityWait::Strict,
            repo,
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
            tx_gate: Arc::new(OnceCell::new()),
            repo_wal: Arc::new(OnceCell::new()),
            per_table_mvcc: Arc::new(scc::HashMap::with_capacity_and_hasher(
                initial_cap,
                THasher::default(),
            )),
            token_names: Arc::new(token_names),
            tx_metrics: Arc::new(shamir_tx::TxMetrics::new()),
            worker_lifecycle: Some(Arc::new(ForegroundLifecycle::new())),
            receipt_worker: Arc::new(OnceCell::new()),
            commit_tail: Arc::new(CommitTailBarrier::default()),
            group_commit: Arc::new(GroupCommit::new()),
            changefeed: Arc::new(OnceCell::new()),
            pending_changefeed_gap: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            journal_publish_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            wal_dir,
            drainer: Arc::new(std::sync::OnceLock::new()),
            repo_interner: Arc::new(OnceCell::new()),
            live: Some(Arc::new(())),
            marker_write_mutex: Arc::new(tokio::sync::Mutex::new(())),
            fk_reverse_cache: Arc::new(FkReverseCache::new()),
            #[cfg(test)]
            table_init_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            wal_group: Arc::new(OnceCell::new()),
            durability_metrics: Arc::new(DurabilityMetrics::default()),
            replication_armed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            durability: Arc::new(arc_swap::ArcSwap::from_pointee(policy)),
            wal_retention_gate: Arc::new(
                crate::tx::clean_horizon::retention_gate::RetentionGate::default(),
            ),
            #[cfg(test)]
            clean_horizon_test_state: Arc::new(std::sync::OnceLock::new()),
            #[cfg(test)]
            background_drainer_disabled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wal_segment_bytes: crate::repo::durability_policy::effective_wal_segment_bytes(),
            write_domain,
        }
    }
}
