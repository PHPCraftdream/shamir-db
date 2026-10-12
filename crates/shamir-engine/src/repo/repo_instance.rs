use super::super::table::{TableConfig, TableManager};
use super::fk_reverse_cache::FkReverseCache;
use super::group_commit::GroupCommit;
use super::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use crate::query::batch::BatchError;
use crate::query::write::WriteResult;
use crate::table::interner_manager::InternerManager;
use crate::tx::CommitTailBarrier;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{fully_unwrap_store, Repo, Store};
use shamir_types::access::Actor;
use shamir_types::types::common::{new_dash_map_wc, TDashMap, THasher};
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

use crate::table::table_manager::table_token_for;

/// Fixed read-side budget, not a tunable: a stalled writer returns an older
/// safe journal prefix after at most one second of admission + confirmation.
/// Barriers share the bounded writer channel's slots with journal events.
const JOURNAL_CONFIRM_BUDGET: Duration = Duration::from_secs(1);

/// Manages a single repository and its tables
pub struct RepoInstance {
    name: String,
    repo: BoxRepo,
    configs: Arc<TDashMap<String, TableConfig>>,
    tables: Arc<TDashMap<String, Arc<OnceCell<TableManager>>>>,
    /// Lazy-initialized RepoTxGate. Created on first call to `tx_gate()`.
    tx_gate: Arc<OnceCell<Arc<shamir_tx::RepoTxGate>>>,
    /// Lazy-initialized RepoWalManager. Created on first call to `repo_wal()`.
    repo_wal: Arc<OnceCell<Arc<shamir_tx::RepoWalManager>>>,
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
    /// On-disk backing directory for this repo, captured from the factory's
    /// `backing_dir()` at construction. `Some(dir)` for disk-backed repos —
    /// [`repo_wal`](Self::repo_wal) builds a file-backed `WalGroupCommit`
    /// rooted at `dir/wal`. `None` for in-memory/test repos — the WAL falls
    /// back to KV markers. INERT plumbing (W3): the file WAL is constructed
    /// but not yet written by the live commit path (W4/W5).
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
    /// tail to `wake()`), and init does no `.await` — it only constructs the
    /// drainer + `tokio::spawn`s the loop. Single-init is guaranteed by
    /// `OnceLock::get_or_init`: the closure runs EXACTLY once even under
    /// concurrent callers (the loser waits for the winner's value and
    /// returns the same `Arc<Drainer>`), so there is no "lost race" — every
    /// caller observes the one-and-only drainer.
    drainer: Arc<std::sync::OnceLock<Arc<crate::tx::drainer::Drainer>>>,
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
        Arc<arc_swap::ArcSwapOption<super::tests::table_init_retry_tests::TableInitHook>>,
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
            repo_interner: Arc::clone(&self.repo_interner),
            // Foreground clone: carries a strong liveness ref. If `self` is
            // itself the background clone (`None`) this stays `None` — a clone
            // of the background handle must not resurrect liveness.
            live: self.live.clone(),
            marker_write_mutex: Arc::clone(&self.marker_write_mutex),
            fk_reverse_cache: Arc::clone(&self.fk_reverse_cache),
            #[cfg(test)]
            table_init_hook: Arc::clone(&self.table_init_hook),
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

    fn from_box_repo_with_wal_dir(
        name: String,
        repo: BoxRepo,
        configs: Vec<TableConfig>,
        wal_dir: Option<std::path::PathBuf>,
    ) -> Self {
        let configs_map: TDashMap<String, TableConfig> = new_dash_map_wc(configs.len().max(16));
        let initial_cap = configs.len().max(16);
        let token_names: scc::HashMap<u64, String, THasher> =
            scc::HashMap::with_capacity_and_hasher(initial_cap, THasher::default());
        for cfg in configs {
            register_token(&token_names, &cfg.name);
            configs_map.insert(cfg.name.clone(), cfg);
        }

        let tables: TDashMap<String, Arc<OnceCell<TableManager>>> = new_dash_map_wc(100);

        Self {
            name,
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
        }
    }

    pub(crate) fn commit_tail(&self) -> &Arc<CommitTailBarrier> {
        &self.commit_tail
    }

    /// cancel-safe: yes — timeout or cancellation only stops this wait.
    /// Stop commit admission first and keep the runtime alive until this returns.
    /// Includes inline, detached and separately spawned AsyncIndex commit tails.
    /// This does not drain WAL history and must not be called by `flush_buffers`
    /// while commits are running.
    pub async fn quiesce_commits(&self, timeout: std::time::Duration) -> DbResult<()> {
        tokio::time::timeout(timeout, self.commit_tail.wait_zero())
            .await
            .map_err(|_| {
                DbError::Internal(format!(
                    "commit quiescence timed out after {timeout:?} for repo '{}'",
                    self.name
                ))
            })
    }

    /// Audit §1.7: mutex guarding the recovery-marker disk write. See the
    /// field doc on [`marker_write_mutex`](Self) for why this exists.
    pub(crate) fn marker_write_mutex(&self) -> &Arc<tokio::sync::Mutex<()>> {
        &self.marker_write_mutex
    }

    /// D2 P1d-2b: produce a BACKGROUND clone for the drain task to own.
    /// Identical to a foreground [`Clone`] except `live = None`, so the task's
    /// owned handle does NOT count toward repo liveness — the loop exits and
    /// the inner `Arc`s are released once the last foreground clone drops.
    /// See [`Drainer::spawn`](crate::tx::drainer::Drainer::spawn).
    fn clone_for_background(&self) -> Self {
        let mut bg = self.clone();
        bg.live = None;
        bg
    }

    /// D2 P1d-2b: the repo's background drainer, spawned once (single-owner).
    ///
    /// First call lazily creates the [`Drainer`](crate::tx::drainer::Drainer)
    /// and starts its background loop (a leak-free task owning a
    /// [`clone_for_background`](Self::clone_for_background) of this repo plus a
    /// `Weak<()>` of [`live`](Self::live)). Subsequent calls (from any
    /// foreground clone) return the same `Arc<Drainer>` via the shared
    /// `OnceCell`. The commit path calls `repo.drainer().wake()` after
    /// publishing a version to nudge the loop.
    pub fn drainer(&self) -> Arc<crate::tx::drainer::Drainer> {
        Arc::clone(self.drainer.get_or_init(|| {
            let drainer = Arc::new(crate::tx::drainer::Drainer::new());
            // 50 ms backstop: prompt drain even if a wake is missed, cheap
            // when idle (the loop short-circuits when nothing is undrained).
            const DRAIN_INTERVAL_MS: u64 = 50;
            let live_weak = match &self.live {
                Some(l) => Arc::downgrade(l),
                // Background clone asking for the drainer (shouldn't happen on
                // the commit path) — a dead weak makes the loop exit at once.
                None => std::sync::Weak::new(),
            };
            drainer.spawn(
                self.clone_for_background(),
                live_weak,
                std::time::Duration::from_millis(DRAIN_INTERVAL_MS),
            );
            drainer
        }))
    }

    /// cancel-safe: NO — recovery can apply durable history and certify a gap
    /// before cancellation. A failed/cancelled open exposes no instance; the
    /// next open retries the idempotent recovery and certification protocol.
    ///
    /// Create, recover and durably certify before returning to ANY caller.
    pub async fn from_factory(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
    ) -> DbResult<Self> {
        #[cfg(test)]
        let fixture = super::tests::recovery_gap_marker_tests::take_open_fixture(&name);
        #[cfg(test)]
        let instance = match fixture {
            Some(instance) => instance,
            None => Self::from_factory_unrecovered(name, factory, configs).await?,
        };
        #[cfg(not(test))]
        let instance = Self::from_factory_unrecovered(name, factory, configs).await?;
        // Exclusive factory open initializes legacy identity before registry
        // publication. All later cloned handles share marker_write_mutex.
        instance.repo_identity().await?;
        instance.recover_v2_inflight().await?;
        Ok(instance)
    }

    /// Unrecovered disk handle solely for tests that explicitly exercise replay
    /// or must install a fault before recovery. Not available in production.
    #[cfg(test)]
    pub(crate) async fn from_factory_for_recovery_test(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
    ) -> DbResult<Self> {
        Self::from_factory_unrecovered(name, factory, configs).await
    }

    async fn from_factory_unrecovered(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
    ) -> DbResult<Self> {
        // Capture the disk backing dir BEFORE consuming the factory, so a
        // disk-backed repo gets a file-WAL group (W3, inert plumbing).
        let wal_dir = factory.backing_dir();
        let repo = factory.create().await?;
        Ok(Self::from_box_repo_with_wal_dir(
            name, repo, configs, wal_dir,
        ))
    }

    /// Repository name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return a clone of this `RepoInstance` with its `name` field replaced
    /// by `new_name`. Used by [`DbInstance::rename_repo`] (Phase F.3) to
    /// re-key a repository under a new logical name without touching any of
    /// its physical table stores — the tables travel "for free" because they
    /// are keyed by table name *inside* the repo, not by the repo name.
    pub fn with_name(&self, new_name: String) -> Self {
        let mut cloned = self.clone();
        cloned.name = new_name;
        cloned
    }

    /// Per-table MvccStore map used by the commit pipeline to route
    /// data writes through version-aware storage.
    pub fn per_table_mvcc(&self) -> &Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>> {
        &self.per_table_mvcc
    }

    /// F-28 Step 4 (#831): this repo's cached reverse-FK map. See
    /// [`FkReverseCache`] for the cache-aside population + invalidation
    /// contract. Used by `query::batch::fk_restrict`/`fk_actions`'s
    /// discovery functions (via `TableResolver::resolve_repo`) and by
    /// `ShamirDb::compile_table_schema`'s invalidation call.
    pub fn fk_reverse_cache(&self) -> &Arc<FkReverseCache> {
        &self.fk_reverse_cache
    }
    /// Atomic transaction telemetry counters.
    pub fn tx_metrics(&self) -> &Arc<shamir_tx::TxMetrics> {
        &self.tx_metrics
    }

    /// Read the unchanged journal, attaching name bindings only for Pull.
    /// The tuple retains the journal's gap metadata; translation errors propagate.
    pub async fn read_changelog_for_replication(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<(Vec<crate::tx::ReplicatedEvent>, Option<u64>)> {
        let journal = self.read_changelog_from(from_version, limit).await?;
        let manager = self.repo_interner().await?;
        let interner = manager.get().await?;
        let mut events = Vec::with_capacity(journal.events.len());
        for event in journal.events {
            let mut ids = shamir_collections::TFxMap::default();
            for change in &event.changes {
                if change.op == shamir_tx::ChangeOp::Put {
                    let bytes = change
                        .value
                        .as_ref()
                        .ok_or_else(|| DbError::Codec("replication Put has no value".into()))?;
                    let value = shamir_types::types::value::InnerValue::from_bytes(bytes)
                        .map_err(|error| DbError::Codec(error.to_string()))?;
                    shamir_tx::collect_referenced_ids(&value, &mut ids);
                }
            }
            let mut field_bindings = Vec::with_capacity(ids.len());
            for id in ids.keys() {
                let name = interner
                    .get_str(&shamir_types::core::interner::InternerKey::new(*id))
                    .ok_or_else(|| {
                        DbError::Codec(format!("replication field id {id} has no name"))
                    })?;
                field_bindings.push((*id, name.to_string()));
            }
            field_bindings.sort_unstable_by_key(|(id, _)| *id);
            events.push(crate::tx::ReplicatedEvent {
                event,
                field_bindings,
            });
        }
        Ok((events, journal.gap_at))
    }

    /// cancel-safe: yes — uses `OnceCell::get_or_try_init`. On cancel
    /// inside the init closure, the cell remains empty so subsequent
    /// calls retry; no partial table is exposed. The clone of the cell
    /// value is in-memory.
    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        // Clone the `Arc<OnceCell>` out of the DashMap and DROP the shard
        // guard BEFORE the init `.await`. DashMap shards are backed by a
        // synchronous `RwLock`; holding the `entry()` write guard across the
        // long-running `create_table_context().await` (which itself awaits
        // store_get / tx_gate / changefeed init) is a guard-across-await
        // deadlock: under runtime oversubscription every worker thread can
        // become wedged on the OS `RwLock` of a shard whose guard-holder is
        // parked at an `.await`, and a synchronous lock cannot yield. The
        // `OnceCell` itself provides the single-init serialization — the
        // shard lock only needs to protect the map insert, not the init.
        let cell = Arc::clone(
            self.tables
                .entry(table_name.to_string())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .value(),
        );

        // §B13: existence-check happens INSIDE the init closure, so it
        // is serialized with the actual context construction. Doing the
        // check up-front would race with concurrent `remove_table`
        // between our `configs.contains_key` and the `tables.entry`
        // install (two independent DashMaps). On a removed table the
        // init returns Err and `OnceCell::get_or_try_init` leaves the
        // cell empty so subsequent calls retry.
        let tbl = cell
            .get_or_try_init(|| async {
                if !self.configs.contains_key(table_name) {
                    return Err(DbError::NotFound(format!(
                        "Table '{}' is not configured in this repository",
                        table_name
                    )));
                }
                self.create_table_context(table_name, &cell).await
            })
            .await?
            .clone();

        // Close the insertion-to-OnceCell-completion window against DROP.
        // No map guard survives an await; rollback can only evict our Arc.
        let current = self.tables.get(table_name);
        if !self.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), &cell))
        {
            if let Some(mvcc) = tbl.mvcc_store() {
                let _ = self
                    .per_table_mvcc
                    .remove_if_sync(&table_token_for(table_name), |existing| {
                        Arc::ptr_eq(existing, &mvcc)
                    });
            }
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' was removed during initialization"
            )));
        }
        Ok(tbl)
    }

    /// Test-only fault-injection seam: force-overwrite the cached
    /// `TableManager` for `table_name` with a caller-supplied instance
    /// (e.g. one built via [`TableManager::create`] on a fault-injecting
    /// `Store` double), so subsequent [`get_table`](Self::get_table) calls
    /// on THIS repo instance (and any clone sharing the same `tables` map)
    /// observe it. Used by `shamir-db`'s resource_meta fail-closed
    /// regression test (audit #540) to make a real catalogue read return
    /// `Err` without a large `BoxRepo`/`BoxRepoFactory` fault-injection
    /// rewrite.
    ///
    /// Gated by the `test-util` feature (NOT plain `#[cfg(test)]`) because
    /// the caller is a downstream crate's test (`shamir-db`) — a
    /// `#[cfg(test)]` item is private to the crate being compiled in test
    /// mode and would not exist when `shamir-engine` is pulled in as a
    /// normal (non-test) dependency. Never compiled into a production
    /// build: no default feature enables `test-util`.
    ///
    /// **Caveat for reuse**: this only overwrites the `tables` cache, NOT
    /// `configs`. Safe when `table_name` is already registered in `configs`
    /// (true for every existing caller — e.g. `SystemStore::init` registers
    /// `"databases"` before this is ever called). Calling this with a
    /// table name that is NOT already in `configs` would desync
    /// `has_table`/`list_table_names`/`token_names` from `tables` — if a
    /// future caller needs that, register the table via the normal config
    /// path first.
    #[cfg(feature = "test-util")]
    pub fn install_table_for_test(&self, table_name: &str, tbl: TableManager) {
        self.tables.insert(
            table_name.to_string(),
            Arc::new(OnceCell::new_with(Some(tbl))),
        );
    }

    /// cancel-safe: yes — all awaits precede MVCC publication. After insert,
    /// only synchronous ownership checks and builders remain.
    async fn create_table_context(
        &self,
        table_name: &str,
        cell: &Arc<OnceCell<TableManager>>,
    ) -> DbResult<TableManager> {
        let data_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__data__{}", table_name))
            .await?;
        let info_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__info__{}", table_name))
            .await?;
        let history_store: Arc<dyn Store> = self
            .repo
            .store_get(format!("__history__{}", table_name))
            .await?;

        let gate = self.tx_gate().await?;

        // This seam models a failure/cancellation in setup that used to run
        // AFTER registration. It is per-repo and consumed once, never global.
        #[cfg(test)]
        if let Some(hook) = self.table_init_hook.swap(None) {
            hook.run().await?;
        }

        // Complete ALL asynchronous/fallible setup before publication.
        // __data__ is inert under MVCC; __info__ keeps its wrappers.
        let data_store = fully_unwrap_store(&data_store).await;
        let tbl = TableManager::create(table_name.to_string(), data_store, info_store).await?;
        let repo_interner = self.repo_interner().await?;
        let mvcc = Arc::new(shamir_tx::MvccStore::new(history_store, Arc::clone(&gate)));

        let token = table_token_for(table_name);
        // A13 follow-up: `insert_sync` returns `Err` when the token is
        // ALREADY present — e.g. a `remove_table(name)` racing this cold
        // init can let a second `create_table_context` run concurrently
        // (see `remove_table`'s own A13 doc comment). Silently discarding
        // that `Err` (the old behavior) leaves the STALE old `MvccStore`
        // wired into the commit pipeline forever while this freshly-built
        // `TableManager` reads through its own (orphaned) `mvcc` handle — a
        // split-brain where committed transactions silently vanish. Fail
        // the table open outright instead: a lost race here means the
        // caller's `OnceCell::get_or_try_init` leaves the cell empty and a
        // retry re-attempts the attach cleanly.
        if self
            .per_table_mvcc
            .insert_sync(token, Arc::clone(&mvcc))
            .is_err()
        {
            log::error!(
                "create_table_context: per_table_mvcc already has an entry for \
                 table '{table_name}' (token {token}) — refusing to attach a second \
                 MvccStore, which would leave a stale store wired into the commit \
                 pipeline while this TableManager reads through a fresh one (A13 \
                 split-brain). Failing the table open; retry will re-attempt the \
                 attach."
            );
            return Err(DbError::Internal(format!(
                "create_table_context: per_table_mvcc attach collision for table \
                 '{table_name}' (token {token}) — a stale MvccStore entry is still \
                 registered"
            )));
        }

        // DROP may have removed/replaced our OnceCell while setup awaited.
        // Existence alone is insufficient after DROP + CREATE of the same name.
        let current = self.tables.get(table_name);
        if !self.configs.contains_key(table_name)
            || !current
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry.value(), cell))
        {
            let _ = self
                .per_table_mvcc
                .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
            return Err(DbError::NotFound(format!(
                "Table '{table_name}' was removed during initialization"
            )));
        }
        let tbl = tbl.with_mvcc_store(mvcc).with_interner(repo_interner);

        // Wire the non-tx write path to the per-repo gate so that
        // Serializable transactions see non-tx writes in their Phase 2-bis
        // predicate-conflict check. The table uses the SAME `gate` the tx
        // commit pipeline uses, keeping non-tx and tx `commit_version`s on
        // one monotonic sequence per repo.
        Ok(tbl.with_changefeed(Arc::clone(&gate)))
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.iter().map(|e| e.key().clone()).collect()
    }

    /// #984 — count degraded (non-`Ready`) indexes across all ALREADY-OPEN
    /// tables in this repo. Pure in-memory — zero store reads. Does NOT
    /// force-open closed tables (that would turn a cheap `/metrics` gauge
    /// into an expensive scan and change server behaviour as a side-effect
    /// of scraping). See [`TableManager::degraded_index_count`] for the
    /// per-table cost model.
    pub async fn degraded_index_count(&self) -> u64 {
        // Collect Arc<OnceCell> clones first to avoid holding DashMap read
        // guards across the `.await` points inside the loop.
        let cells: Vec<Arc<OnceCell<TableManager>>> =
            self.tables.iter().map(|e| Arc::clone(e.value())).collect();
        let mut count = 0u64;
        for cell in &cells {
            if let Some(tm) = cell.get() {
                count += tm.degraded_index_count().await;
            }
        }
        count
    }

    pub fn has_table(&self, table_name: &str) -> bool {
        self.configs.contains_key(table_name)
    }

    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Register a new table in the repository.
    /// The table is lazily created on first access via get_table().
    ///
    /// Also records `table_token_for(name) → name` in the reverse index so
    /// `table_by_token` resolves in O(1) (III.1). Re-registering an already
    /// known name is idempotent.
    ///
    /// F-28 Step 4 (#831): invalidates the cached reverse-FK map — a newly
    /// registered table changes `list_table_names()`'s membership, which
    /// `discover_restrict_refs`/`discover_action_refs` scan when rebuilding
    /// the cache.
    pub fn add_table(&self, config: TableConfig) {
        // Same synchronous DDL lock order as remove_table. This closes the
        // config/reverse-token re-add window without holding guards over await.
        let _entry = self.tables.entry(config.name.clone());
        register_token(&self.token_names, &config.name);
        self.configs.insert(config.name.clone(), config);
        self.fk_reverse_cache.invalidate();
    }

    /// Remove a table from the repository.
    /// Returns true if the table existed and was removed.
    ///
    /// F-28 Step 4 (#831): invalidates the cached reverse-FK map on an
    /// actual removal — same rationale as `add_table` (table membership
    /// changed).
    pub fn remove_table(&self, table_name: &str) -> bool {
        // DDL-only synchronous critical section: tables -> configs -> scc.
        // Keep the table shard exclusive until owned MVCC cleanup completes,
        // so a replacement initializer cannot publish in the removal window.
        let entry = self.tables.entry(table_name.to_string());
        let removed = self.configs.remove(table_name).is_some();
        if removed {
            self.fk_reverse_cache.invalidate();
            let token = table_token_for(table_name);
            let _ = self
                .token_names
                .remove_if_sync(&token, |existing| existing.as_str() == table_name);
            if let dashmap::mapref::entry::Entry::Occupied(entry) = &entry {
                if let Some(mvcc) = entry.get().get().and_then(TableManager::mvcc_store) {
                    let _ = self
                        .per_table_mvcc
                        .remove_if_sync(&token, |existing| Arc::ptr_eq(existing, &mvcc));
                }
            }
            // An empty/in-flight cell owns no published MVCC. Its initializer
            // must detect retirement and roll back only its own registration.
            if let dashmap::mapref::entry::Entry::Occupied(entry) = entry {
                entry.remove();
            }
        }
        removed
    }

    /// Rename a table's live registration: copies the three physical
    /// data stores (`__data__`, `__info__`, `__history__`) from the old
    /// name to the new one via [`Repo::copy_store`], then swaps the
    /// in-memory config + reverse-index entry. Returns `false` if the
    /// source table was not configured.
    ///
    /// The OLD physical stores are intentionally orphaned (same
    /// disposition as `drop_table`, which orphans `__data__` because
    /// the catalogue is the source of truth). If the caller also needs
    /// the old stores gone, follow up with `store_delete` per namespace.
    pub async fn rename_table_stores(&self, from: &str, to: &str) -> DbResult<bool> {
        // F-68 (#895) cluster D / task #124 diagnostic instrumentation:
        // timestamped entry/exit — NOT a behavior change, logging only.
        let rts_started = std::time::Instant::now();
        log::debug!("rename_table_stores: enter from={from} to={to}");
        // Snapshot the old config BEFORE mutating `configs` so a missing
        // table is reported cleanly and a concurrent `remove_table` can
        // only make us return `false` (no half-applied rename).
        let old_config = match self.configs.get(from) {
            Some(c) => c.clone(),
            None => return Ok(false),
        };

        // 0. Force-drain the source table's MVCC overlay into
        //    `__history__<from>` so the copy picks up EVERY committed row.
        //    `drain_to_history` snapshots the overlay up to the visibility
        //    watermark, writes each version through
        //    `write_committed_to_history`, advances the durable watermark,
        //    and reclaims the overlay — synchronously. Without this, rows
        //    that still live only in the in-memory overlay (not yet drained
        //    by the background drainer) would be missed by the store copy.
        //    Idempotent: if the overlay is already drained (all entries are
        //    in history), this is a no-op.
        let from_token = table_token_for(from);
        // Clone the Arc out: `get_sync`'s entry guard holds the bucket lock,
        // and the drainer's `iter_sync` over this map parks its thread on it,
        // so holding it across the awaits below wedges a current-thread
        // runtime (#589 class).
        let from_mvcc = self
            .per_table_mvcc
            .read_sync(&from_token, |_, mvcc| Arc::clone(mvcc));
        if let Some(mvcc) = from_mvcc {
            // F-68 (#895) cluster D / task #124: timestamped before/after
            // around `drain_to_history` — this synchronously walks the
            // overlay and writes each drained version to `__history__`, the
            // one potentially I/O-heavy step in this function.
            let drain_started = std::time::Instant::now();
            log::debug!("rename_table_stores: calling drain_to_history from={from}");
            mvcc.drain_to_history().await?;
            log::debug!(
                "rename_table_stores: drain_to_history returned after {:?}",
                drain_started.elapsed()
            );
        }

        // 1. Copy physical stores under the new name. `copy_store` is
        //    idempotent on an empty destination (a fresh store).
        self.repo
            .copy_store(&format!("__data__{}", from), &format!("__data__{}", to))
            .await?;
        self.repo
            .copy_store(&format!("__info__{}", from), &format!("__info__{}", to))
            .await?;
        self.repo
            .copy_store(
                &format!("__history__{}", from),
                &format!("__history__{}", to),
            )
            .await?;

        // 2. Drop the old live registration. `remove_table` also clears
        //    the in-memory `OnceCell<TableManager>`, the old
        //    `token_names` reverse-index entry, AND the old
        //    `per_table_mvcc` registration (A13) so a future CREATE under
        //    the `from` name re-registers cleanly.
        let _ = self.remove_table(from);

        // 3. Register the new table config (preserving `enable_indexes`)
        //    and install its reverse-index entry. Reuses `add_table` so
        //    the new `OnceCell` lazily constructs a fresh `TableManager`
        //    pointing at the just-copied stores on first access.
        let new_config = TableConfig {
            name: to.to_string(),
            enable_indexes: old_config.enable_indexes,
        };
        self.add_table(new_config);

        // F-68 (#895) cluster D / task #124: exit timestamp, paired with
        // the "enter" log above.
        log::debug!(
            "rename_table_stores: exit from={from} to={to} elapsed={:?}",
            rts_started.elapsed()
        );

        Ok(true)
    }

    /// Cold metadata operation; clones serialize read/create/flush with the
    /// same async mutex. No synchronous guard crosses an await.
    pub async fn repo_identity(&self) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::load_or_create(&store).await
    }

    pub async fn bump_history_epoch(&self) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::bump(&store, None).await
    }

    /// Retry-safe even when a previous open bumped before marker removal.
    pub async fn apply_restore_operation(
        &self,
        operation: [u8; 16],
    ) -> DbResult<crate::meta::repo_identity::RepoIdentity> {
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        crate::meta::repo_identity::RepoIdentity::bump(&store, Some(operation)).await
    }

    pub async fn certified_gap_interval(&self) -> DbResult<Option<(u64, u64)>> {
        Ok(self.changefeed().await?.feed.certified_gap_interval())
    }

    /// Returns the repo's transactional info_store under the
    /// `"__tx__"` namespace.
    ///
    /// Shared with [`tx_gate`](Self::tx_gate) and
    /// [`repo_wal`](Self::repo_wal). The commit pipeline writes
    /// recovery markers (`MetaKey::LastCommittedVersion`,
    /// `MetaKey::NextTxId`) here in Phase 6.5 (see
    /// `crate::tx::commit`) so a clean restart can seed the gate
    /// without scanning every active WAL marker.
    pub async fn tx_info_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__tx__").await
    }

    /// Install a fault-injecting manager before any repo/table interner access.
    /// Never replace an initialized manager (including one shared by clones).
    #[cfg(test)]
    pub(crate) fn install_repo_interner_for_test(&self, manager: InternerManager) {
        assert!(
            self.repo_interner.set(manager).is_ok(),
            "repo interner must be uninitialized before test injection",
        );
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init the cell stays empty and the next
    /// caller retries. Returned manager is a cheap clone (all internal Arcs
    /// are shared).
    ///
    /// The per-repo [`InternerManager`] (Stage I — moved from per-table to
    /// per-repo ownership). Backed by the dedicated `"__interner__"` store,
    /// distinct from `"__tx__"` (recovery markers) so the two durability
    /// streams — chunked interner deltas vs. last-committed/next-tx markers —
    /// never collide. Built once on first access; every
    /// [`TableManager`](crate::table::TableManager) shares this one manager
    /// via `with_interner`, so a field name resolves to ONE id across all
    /// tables in the repo. V2 WAL recovery resolves this single interner
    /// instead of a per-table one (Stage I keystone).
    pub async fn repo_interner(&self) -> DbResult<InternerManager> {
        self.repo_interner
            .get_or_try_init(|| async {
                let store: Arc<dyn Store> = self.repo.store_get("__interner__").await?;
                Ok::<InternerManager, DbError>(InternerManager::new(store))
            })
            .await
            .cloned()
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init, the cell remains uninitialised
    /// and the next caller retries. Returned Arc clone is in-memory.
    ///
    /// Returns the per-repo transaction gate, lazily initialising it on
    /// first call.
    ///
    /// The gate is seeded from durable recovery markers stored under the
    /// repo's `"__tx__"` info store:
    /// - `last_committed_version` from `MetaKey::LastCommittedVersion`
    /// - `next_tx_id` from `MetaKey::NextTxId`
    ///
    /// Recovery marker reads are best-effort — if absent we start from
    /// defaults (`RepoTxGate::fresh()`).
    ///
    /// CRIT-B: the persisted `last_committed_version` marker can lag the
    /// highest `commit_version` of an *inflight* V2 WAL entry — the
    /// commit pipeline stamps `commit_version` in Phase 4 (WAL begin) but
    /// only persists the marker in Phase 6.5. A crash in that window
    /// leaves the marker stale (e.g. 7) while a durable inflight entry
    /// carries `commit_version = 10`. Seeding the gate solely from the
    /// marker would let `assign_next_version()` re-issue 8, 9, 10 —
    /// versions the crashed (and about-to-be-recovered) tx already
    /// consumed — violating version monotonicity. We therefore pre-scan
    /// the inflight entries and seed the gate's counter from
    /// `max(marker, max_inflight_commit_version)`. `RepoTxGate::new`
    /// initialises BOTH the monotonic `version_counter` and
    /// `last_committed_version` to this floor, so the next
    /// `assign_next_version()` is strictly greater than every version a
    /// recovered entry will replay at. The scan is the same cheap
    /// `recover()` replay recovery runs and happens once per gate (OnceCell).
    pub async fn tx_gate(&self) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
        self.tx_gate
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;

                let marker = crate::meta::recovery_marker::load_last_committed(&info_store)
                    .await
                    .unwrap_or(None)
                    .unwrap_or(0);
                let next_tx_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);

                // Pre-scan inflight V2 entries so the version floor covers
                // any commit_version stamped before its marker was
                // persisted (CRIT-B). Best-effort: a WAL read error here
                // falls back to the marker rather than blocking gate
                // construction — recovery (which propagates errors) runs
                // separately on the open path.
                let max_inflight = self.max_inflight_commit_version().await.unwrap_or(0);
                let last_committed = marker.max(max_inflight);

                let gate = shamir_tx::RepoTxGate::new(last_committed, next_tx_id);
                Ok::<Arc<shamir_tx::RepoTxGate>, DbError>(Arc::new(gate))
            })
            .await
            .cloned()
    }

    /// Returns the eager latest-state publication floor for this repo.
    ///
    /// Lower allocated versions may still be unsettled. Keep this value for
    /// latest-state/replication diagnostics, not snapshot positions or seeds;
    /// use [`consistent_version`](Self::consistent_version) for those.
    ///
    /// cancel-safe: yes — delegates to cancellation-safe gate initialization.
    pub async fn current_commit_version(&self) -> DbResult<u64> {
        let gate = self.tx_gate().await?;
        Ok(gate.last_committed())
    }

    /// Highest contiguous settled version, suitable for snapshot positions.
    /// Every version through this position is materialized or aborted.
    ///
    /// cancel-safe: yes — delegates to cancellation-safe gate initialization.
    pub async fn consistent_version(&self) -> DbResult<u64> {
        let gate = self.tx_gate().await?;
        Ok(gate.visible_floor())
    }

    /// Highest `commit_version` across all durable inflight V2 WAL
    /// entries, or `0` if there are none. Used to seed the tx gate's
    /// version floor (CRIT-B) so recovered commit versions are never
    /// re-issued.
    pub(crate) async fn max_inflight_commit_version(&self) -> DbResult<u64> {
        let wal = self.repo_wal().await?;
        let entries = wal.recover().await?;
        Ok(entries.iter().map(|e| e.commit_version).max().unwrap_or(0))
    }

    /// cancel-safe: yes — `OnceCell::get_or_try_init` semantics. If
    /// cancellation occurs during init, the cell remains uninitialised
    /// and the next caller retries. Returned Arc clone is in-memory.
    ///
    /// Returns the per-repo WAL manager, lazily initialising it on first
    /// call. Shares the `"__tx__"` info store with [`tx_gate`].
    ///
    /// The `next_txn_id` counter is seeded from
    /// `max(persisted_NextTxId, max_inflight_txn_id + 1)`. The floor is
    /// computed from `recover()` (segment/Mem replay) — empty on a fresh
    /// in-memory instance (safeguard inert), the durable entries on a disk
    /// repo. The append-only segment keys entries by frame position and
    /// recovery replays by the monotonically-floored `commit_version`, so
    /// even a repeated txn_id after a crash carries no ordering ambiguity
    /// or data loss; the floor is a belt-and-braces anti-collision guard.
    pub async fn repo_wal(&self) -> DbResult<Arc<shamir_tx::RepoWalManager>> {
        self.repo_wal
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;
                let initial_txn_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);
                // F5e: build a `WalGroupCommit` for EVERY repo — `File` sink
                // for disk repos (sibling `<name>.shamirwal/` directory via
                // `file_name()` — NOT `OsString::push` on the full path, which
                // breaks on trailing separators), `Mem` sink (in-RAM Vec) for
                // in-memory repos. The group is always present, so the live
                // commit path is one code path: `begin_grouped(Buffered)` to
                // append, `recover()` (segment/Mem `replay()`) to recover.
                // Only disk repos get a background fsync — the Mem sink's
                // `sync` is a no-op, so a timer would be pointless.
                let group = match &self.wal_dir {
                    Some(dir) => {
                        let wal_dir = match dir.file_name() {
                            Some(name) => {
                                let mut n = name.to_os_string();
                                n.push(".shamirwal");
                                dir.with_file_name(n)
                            }
                            None => dir.join("shamirwal"),
                        };
                        let wal_dir_for_blocking = wal_dir.clone();
                        tokio::task::spawn_blocking(move || {
                            std::fs::create_dir_all(&wal_dir_for_blocking)
                        })
                        .await
                        .map_err(|e| DbError::Storage(format!("wal dir join: {e}")))?
                        .map_err(|e| DbError::Storage(format!("wal dir create: {e}")))?;
                        // F6b: the WAL is a DIRECTORY of numbered segments
                        // (`NNNNNNNN.wal`), not a single `repo.wal`. The
                        // project is pre-release with an atomic flip (no
                        // dual-recovery bridge — `wal-refactor.md` §4.4), so
                        // there is no legacy `repo.wal` migration: `SegmentSet`
                        // simply owns `<name>.shamirwal/` and rotates/truncates
                        // segments inside it. `create_dir_all` above already
                        // made the dir; `SegmentSet::open` opens the first
                        // active `00000000.wal`.
                        // F6c: the seal/rotate threshold is the
                        // `WAL_SEGMENT_MAX_BYTES` tunable (8 MiB), overridable
                        // per-process via `SHAMIR_WAL_SEGMENT_MAX_BYTES` (parsed
                        // as `u64`; a malformed value falls back to the const).
                        // This is a legitimate ops tunable (a small cap forces
                        // rotation+seal so truncation/growth-limit tests can
                        // reach segment boundaries without huge data volumes),
                        // NOT a debug-only seam — left live in every build, but
                        // the default is always the const. One `env::var` per
                        // repo open is negligible.
                        let seg_max_bytes = std::env::var("SHAMIR_WAL_SEGMENT_MAX_BYTES")
                            .ok()
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(shamir_tunables::instance_defaults::WAL_SEGMENT_MAX_BYTES);
                        let segset =
                            shamir_wal::SegmentSet::open(wal_dir.clone(), seg_max_bytes).await?;
                        let sink = shamir_wal::WalSink::File(segset);
                        let group = Arc::new(shamir_wal::WalGroupCommit::new(Arc::new(sink)));
                        // RF1: background fsync bounds the power-loss window for
                        // Buffered (level 2) commits. 250 ms = max data-at-risk.
                        const WAL_BG_FSYNC_MS: u64 = 250;
                        group.spawn_background_fsync(std::time::Duration::from_millis(
                            WAL_BG_FSYNC_MS,
                        ));
                        group
                    }
                    None => {
                        let sink = shamir_wal::WalSink::mem();
                        Arc::new(shamir_wal::WalGroupCommit::new(Arc::new(sink)))
                    }
                };
                let mgr = shamir_tx::RepoWalManager::new(initial_txn_id, group);

                // Anti-collision safeguard: floor next_txn_id above any
                // inflight txn_id replayed from the WAL, so a fresh
                // `fresh_txn_id` can never reuse an id a crashed entry already
                // used. On a fresh Mem instance `recover()` is empty → inert;
                // on a disk repo it returns the durable entries.
                if let Ok(entries) = mgr.recover().await {
                    if let Some(max_txn_id) = entries.iter().map(|e| e.txn_id).max() {
                        mgr.seed_floor_at_least(max_txn_id + 1);
                    }
                }

                Ok::<Arc<shamir_tx::RepoWalManager>, DbError>(Arc::new(mgr))
            })
            .await
            .cloned()
    }

    /// cancel-safe: yes — bumps a tx-start counter (atomic), opens a
    /// snapshot (single scc insert under the hood) and returns. If the
    /// caller is dropped mid-await before receiving the guard, the
    /// stack-local guard is dropped which removes the snapshot from the
    /// active set. The only persistent footprint is an atomic counter
    /// increment in `tx_metrics`; tx-ids/version-counters drift safely
    /// (monotonic, no reuse).
    ///
    /// Open a fresh transaction on this repo.
    ///
    /// Returns a `(TxContext, SnapshotGuard)` pair. The guard's lifetime
    /// must extend at least until commit (or drop = rollback) — drop
    /// removes the snapshot from the active set so GC can reclaim
    /// versions older than `min_alive`.
    ///
    /// `repo_id` in the TxContext is populated via [`repo_token`].
    pub async fn begin_tx(
        &self,
        isolation: shamir_tx::IsolationLevel,
    ) -> DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
        self.tx_metrics.on_tx_start();
        let gate = self.tx_gate().await?;
        // Open the snapshot BEFORE constructing TxContext so that any
        // concurrent non-tx write sees active_serializable_count > 0 for
        // the full lifetime of this tx's predicate window.
        let guard = if isolation == shamir_tx::IsolationLevel::Serializable {
            gate.open_snapshot_serializable().await
        } else {
            gate.open_snapshot().await
        };
        let snapshot_version = guard.version();
        let tx_id = gate.fresh_tx_id();
        let mut tx =
            shamir_tx::TxContext::new(tx_id, repo_token(&self.name), snapshot_version, isolation);

        // FG-7: attach the version provider UNCONDITIONALLY (every isolation
        // level), not just Serializable. `pre_commit_locked_validate`'s new
        // Phase CAS needs `version_of` for `expected_version` re-validation
        // at commit regardless of isolation — a Snapshot tx with no CAS
        // checks pays only a thin `Arc` construction + clone, no behavior
        // change to Phase 2 (still gated on `isolation == Serializable`).
        let provider = std::sync::Arc::new(crate::repo::RepoVersionProvider {
            per_table_mvcc: Arc::clone(&self.per_table_mvcc),
        });
        tx.set_version_provider(provider);

        Ok((tx, guard))
    }

    /// cancel-safe: partial — cancellation during prelock (including unique-lock
    /// waits) is still a clean abort. AFTER prelock, cancellation only stops the
    /// caller's wait: the exact suspended commit continues on the runtime, even
    /// before WAL acceptance. Do not blindly retry an unobserved outcome.
    /// Inline panics propagate; detached panics are logged, with VersionGuard
    /// orphan recovery as the fallback. Stop admission and await
    /// [`quiesce_commits`](Self::quiesce_commits) before stopping the runtime.
    /// See [`crate::tx::commit_tx`] for the full contract.
    ///
    /// Commit a transaction via the 7-phase commit pipeline.
    ///
    /// Wrapper around [`crate::tx::commit_tx`]. The free function is
    /// the canonical implementation; this method exposes it on the
    /// natural semantic owner.
    pub async fn commit_tx(
        &self,
        tx: shamir_tx::TxContext,
    ) -> Result<crate::tx::TxOutcome, crate::tx::CommitError> {
        crate::tx::commit_tx(tx, self).await
    }

    /// cancel-safe: NO — multi-step state mutation (local version
    /// allocation + per-table raw writes + changefeed emit). The raw-apply
    /// primitive itself (`MvccStore::apply_committed_ops`) is last-write-
    /// wins idempotent, so a re-delivery of the same event after a crash
    /// converges as long as the caller does NOT advance its
    /// `applied_watermark` past an event that did not return
    /// [`ApplyOutcome::Applied`].
    ///
    /// R1-a — apply a single leader-emitted [`ChangelogEvent`] to this
    /// follower repo as a trusted raw write. Wrapper around
    /// [`crate::tx::apply_replicated`]. See its docs for the version-
    /// allocation model, idempotency contract, and finalize-tail reuse
    /// rationale.
    ///
    /// `applied_watermark` is the highest leader `commit_version` the
    /// caller has durably recorded as applied (R1-b owns the durable
    /// bookmark). The method performs an O(1) comparison and short-
    /// circuits to [`ApplyOutcome::Skipped`] without touching the store
    /// when `event.commit_version <= applied_watermark`.
    ///
    /// `policy` selects the trust model (#1199) — see
    /// [`crate::tx::ApplyPolicy`]'s doc.
    pub async fn apply_replicated(
        &self,
        event: &shamir_tx::ChangelogEvent,
        field_bindings: &[(u64, String)],
        applied_watermark: u64,
        policy: crate::tx::ApplyPolicy,
    ) -> DbResult<crate::tx::ApplyOutcome> {
        crate::tx::apply_replicated(self, event, field_bindings, applied_watermark, policy).await
    }

    /// R1-b — read the durable per-(db,repo) follower replication bookmark.
    ///
    /// Returns the highest LEADER `commit_version` this follower has
    /// durably recorded as applied on this repo. Returns `0` on a fresh
    /// repo (the marker has never been written). The value is what the
    /// follower feeds back into
    /// [`apply_replicated`](Self::apply_replicated) as
    /// `applied_watermark` for O(1) idempotent re-delivery gating.
    ///
    /// Stored in the repo's `__tx__` info-store under
    /// [`MetaKey::ReplicationBookmark`](crate::meta::MetaKey::ReplicationBookmark)
    /// via the same envelope codec as `LastCommittedVersion` / `NextTxId`.
    /// See `crate::meta::repl_bookmark` for the codec rationale and the
    /// separate-marker rationale (leader-version space ≠ local-version
    /// space).
    pub async fn replication_bookmark(&self) -> DbResult<u64> {
        let info_store = self.tx_info_store().await?;
        crate::meta::repl_bookmark::load_replication_bookmark(&info_store).await
    }

    /// R1-b — monotonically advance the durable replication bookmark.
    ///
    /// Persists `version` as the new high-water mark ONLY if `version` is
    /// strictly greater than the currently-persisted bookmark; otherwise
    /// this is a no-op (returns `Ok(())`). The compare-then-swap guards
    /// against an out-of-order delivery rewinding the bookmark: e.g. after
    /// advancing to 5, a late-arriving event at leader-version 3 is rejected
    /// and the bookmark stays at 5.
    ///
    /// Callers should typically advance to the leader `commit_version` of
    /// the event returned as [`crate::tx::ApplyOutcome::Applied`] by
    /// [`apply_replicated`](Self::apply_replicated). On error the bookmark
    /// is unchanged (the persist is atomic at the store level).
    pub async fn advance_replication_bookmark(&self, version: u64) -> DbResult<()> {
        let info_store = self.tx_info_store().await?;
        let current = crate::meta::repl_bookmark::load_replication_bookmark(&info_store).await?;
        if version > current {
            crate::meta::repl_bookmark::save_replication_bookmark(&info_store, version).await?;
        }
        Ok(())
    }

    /// Run a single non-tx write as an implicit single-op BATCH transaction.
    ///
    /// F4b-1 keystone of "everything is a transaction": instead of taking the
    /// direct V1 write path (which emits `begin_with_delta`/`commit` WAL
    /// markers), a non-tx write opens a [`IsolationLevel::Snapshot`] tx on
    /// this repo, stages the write via the `_tx` execute path (`stage`), and
    /// commits — folding the data, index postings, and counter into ONE
    /// `WalEntryV2` and consuming ONE commit version.
    ///
    /// Snapshot isolation is deliberate: SSI validation is gated on
    /// `Serializable`, so the implicit tx NEVER aborts on a read/write
    /// conflict — this preserves non-tx last-writer-wins semantics.
    /// Unique-index violations still surface (the unique re-validation in
    /// pre-commit is unconditional) and are mapped to a coded
    /// `unique_violation` error.
    ///
    /// On a `stage` error or a commit error the `TxContext` / `SnapshotGuard`
    /// drop = clean RAII abort.
    ///
    /// `stage` receives the open `&mut TxContext` and returns the write's
    /// [`WriteResult`]; the returned ids / shape are identical to the direct
    /// path.
    ///
    /// F5a: lifted from the private `run_implicit_batch_tx` free fn in
    /// `query::batch::query_runner` so the `shamir-db` system-store and
    /// admin user/role direct-delete callers can route their deletes through
    /// the same implicit-tx file-WAL path (retiring the V1 DELETE marker).
    ///
    /// F-28 Step 1 (#828): reimplemented as a thin wrapper over
    /// [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx) /
    /// [`commit_implicit_batch_tx`](Self::commit_implicit_batch_tx) — the
    /// `stage` HRTB closure this method still accepts forces any captured
    /// reference to satisfy `'a: 'static` (see those methods' docs), which is
    /// why `query_runner`'s 4 implicit-arm call sites moved off this API onto
    /// the split begin/commit pair directly. This wrapper is kept, unchanged
    /// in signature and behavior, for its ~14 other existing callers.
    ///
    /// The two delegated calls are `Box::pin`'d: without it, this debug
    /// (`opt-level = 0`) build inlines `begin_implicit_batch_tx`'s and
    /// `commit_implicit_batch_tx`'s full `async fn` state machines into
    /// THIS future's state machine on top of the original inline body,
    /// growing every caller's stack frame by the sum of both — measured to
    /// tip at least one pre-existing, unrelated `#[tokio::test]` deep in a
    /// DDL admin call chain (`access_ddl::group_grant_via_ddl`) over the
    /// default Windows thread stack, observed as a `has overflowed its
    /// stack` abort with no logic change on that path at all. Boxing moves
    /// each callee's state machine to the heap, keeping this fn's own frame
    /// (and thus every one of `run_implicit_batch_tx`'s ~14 callers) at
    /// pre-refactor size — verified by reproducing the failure/fix with the
    /// delegation form isolated in a scratch worktree before landing this.
    pub async fn run_implicit_batch_tx<F>(
        &self,
        actor: Actor,
        alias: &str,
        stage: F,
    ) -> Result<WriteResult, BatchError>
    where
        F: for<'t> FnOnce(
            &'t mut shamir_tx::TxContext,
        )
            -> Pin<Box<dyn Future<Output = DbResult<WriteResult>> + Send + 't>>,
    {
        let (mut tx, _guard) = Box::pin(self.begin_implicit_batch_tx(actor, alias)).await?;

        // Stage the write into the tx. On error drop tx/_guard = RAII abort.
        let wr = stage(&mut tx).await.map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: e.to_string(),
            code: e.code().map(str::to_owned),
        })?;

        // Commit — folds everything into one WalEntryV2 / one commit_version.
        Box::pin(self.commit_implicit_batch_tx(tx, alias)).await?;
        Ok(wr)
    }

    /// Open the implicit Snapshot batch tx exactly as `run_implicit_batch_tx`
    /// does internally (set_actor + set_implicit(true)), returning the tx and
    /// its SnapshotGuard so the CALLER can stage with ordinary straight-line
    /// code that borrows freely (no HRTB) — see F-28 Step 1 (#828).
    ///
    /// The returned `TxContext` / `SnapshotGuard` are plain local variables:
    /// both drop cleanly (RAII abort) on any early return/error the caller
    /// hits between this call and [`commit_implicit_batch_tx`], exactly as
    /// `run_implicit_batch_tx`'s internal `tx`/`_guard` did.
    pub async fn begin_implicit_batch_tx(
        &self,
        actor: Actor,
        alias: &str,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        self.begin_implicit_batch_tx_with_isolation(
            actor,
            alias,
            shamir_tx::IsolationLevel::Snapshot,
        )
        .await
    }

    /// Like [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx), but
    /// lets the caller pick the isolation level instead of hardcoding
    /// `Snapshot`.
    ///
    /// F-28 Step 5 (S3-C): the query-runner's implicit DELETE/UPDATE arms use
    /// this to upgrade to `Serializable` for a single implicit tx when the
    /// target table is flagged (`FkReverseCache::is_fk_parent_with_delete_action`
    /// / `is_fk_parent_with_update_action`) as an FK parent with a
    /// non-`NoAction` `on_delete`/`on_update` — the Serializable predicate
    /// check is what closes the cross-transaction TOCTOU race between the FK
    /// reverse-check scan and this tx's commit
    /// (see `fk_restrict.rs`/`fk_actions.rs`/`fk_on_update.rs`'s module docs).
    /// Every other implicit-tx caller keeps calling
    /// [`begin_implicit_batch_tx`](Self::begin_implicit_batch_tx), which
    /// delegates here with `Snapshot` — byte-identical behavior, unchanged.
    pub async fn begin_implicit_batch_tx_with_isolation(
        &self,
        actor: Actor,
        alias: &str,
        isolation: shamir_tx::IsolationLevel,
    ) -> Result<(shamir_tx::TxContext, shamir_tx::SnapshotGuard), BatchError> {
        let (mut tx, guard) =
            self.begin_tx(isolation)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("implicit begin_tx: {}", e),
                    code: None,
                })?;
        // Provenance: thread the actor for commit-time attribution (R2).
        tx.set_actor(actor);
        // Mark implicit so the changefeed event reports tx_id == 0 (preserving
        // the "0 = non-tx write" subscription contract). The internal tx_id
        // stays real for WAL / crash-injection seams.
        tx.set_implicit(true);
        Ok((tx, guard))
    }

    /// Commit an implicit batch tx with the canonical BatchError mapping —
    /// same error-precedence/coding as `run_implicit_batch_tx`'s existing
    /// commit-error handling (UniqueViolation -> "unique_violation",
    /// CasConflict -> "version_conflict" per FG-7). See F-28 Step 1 (#828).
    pub async fn commit_implicit_batch_tx(
        &self,
        tx: shamir_tx::TxContext,
        alias: &str,
    ) -> Result<(), BatchError> {
        match self.commit_tx(tx).await {
            Ok(_outcome) => Ok(()),
            Err(commit_err) => {
                let (message, code) = match commit_err {
                    crate::tx::CommitError::MigrationFrozen { .. } => {
                        (commit_err.to_string(), Some("table_migrating".to_string()))
                    }
                    crate::tx::CommitError::UniqueViolation { .. } => {
                        (commit_err.to_string(), Some("unique_violation".to_string()))
                    }
                    // FG-7: a commit-time CAS failure on the implicit
                    // (plain non-transactional) Snapshot path must surface
                    // the SAME "version_conflict" code as the immediate
                    // staging-time check — mirrors the identical mapping in
                    // `batch_execute.rs` / `db_tx.rs`.
                    crate::tx::CommitError::CasConflict { .. } => {
                        (commit_err.to_string(), Some("version_conflict".to_string()))
                    }
                    // F-28 Step 5: an implicit tx upgraded to Serializable
                    // (FK-parent delete/update fan-out) can now abort with a
                    // genuine SSI conflict — code it the same as the explicit
                    // transactional-batch path (`batch_execute.rs`) does, so
                    // client retry logic keyed on "tx_conflict" covers this
                    // path too.
                    crate::tx::CommitError::PhantomConflict { .. }
                    | crate::tx::CommitError::SsiConflict { .. } => {
                        (commit_err.to_string(), Some("tx_conflict".to_string()))
                    }
                    crate::tx::CommitError::WalRequiresReopen => (
                        commit_err.to_string(),
                        Some("wal_requires_reopen".to_string()),
                    ),
                    crate::tx::CommitError::WalDurability { .. } => {
                        (commit_err.to_string(), Some("wal_durability".to_string()))
                    }
                    other => (other.to_string(), None),
                };
                Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message,
                    code,
                })
            }
        }
    }

    pub(crate) async fn mark_changefeed_gap(&self, version: u64) -> DbResult<()> {
        self.changefeed().await?.feed.mark_gap(version);
        Ok(())
    }

    // ============================================================================
    // Changefeed (Phase 3b): live broadcast + durable journal
    // ============================================================================

    /// cancel-safe: yes — `OnceCell::get_or_try_init`. If cancellation
    /// occurs during init the cell stays empty and the next caller retries;
    /// the returned handle is an in-memory `Arc` clone.
    ///
    /// Lazily build (once per repo) the changefeed: a live broadcast sender
    /// plus a background journal writer over the `"__changelog__"` store.
    /// The feed is "always on" — the commit-path feeds the journal on every
    /// non-empty commit regardless of live subscribers — so callers that
    /// subscribed late can still catch up via [`read_changelog_from`].
    ///
    /// [`read_changelog_from`]: Self::read_changelog_from
    pub(crate) async fn changefeed(&self) -> DbResult<ChangefeedHandle> {
        let handle = self
            .changefeed
            .get_or_try_init(|| async {
                #[cfg(test)]
                if let Some(hook) = self.journal_publish_hook.load_full() {
                    if hook
                        .init_failures
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                        .is_ok()
                    {
                        return Err(DbError::Storage("injected changefeed init failure".into()));
                    }
                }
                let store: Arc<dyn Store> = self.repo.store_get("__changelog__").await?;
                let adapter = crate::repo::changelog_store::StoreChangelog::new(store);
                let clean_horizon = adapter.load_clean_horizon().await?;
                let (marker, repair_marker) = adapter.load_gap_marker().await?;
                let cl_store: Arc<dyn shamir_tx::ChangelogStore> = Arc::new(adapter);
                let pending = self.pending_changefeed_gap.load(Ordering::Acquire);
                let restored = match (marker, pending) {
                    (m, 0) => m,
                    (None, p) => Some((p, p)),
                    (Some((first, through)), p) => Some((first.min(p), through.max(p))),
                };
                #[cfg(test)]
                if let Some(hook) = self.journal_publish_hook.load_full() {
                    if hook.init_snapshot_armed.swap(false, Ordering::AcqRel) {
                        hook.reached.notify_one();
                        hook.resume.notified().await;
                    }
                }
                // Seeds claim durability: flush pending/corrupt metadata before
                // spawning the sole writer. Failed init leaves pending intact.
                if repair_marker || restored != marker {
                    if let Some(interval) = restored {
                        cl_store
                            .put(
                                shamir_tx::changefeed::gap_marker_key(),
                                shamir_tx::changefeed::encode_gap_marker(interval),
                            )
                            .await
                            .map_err(DbError::Storage)?;
                    }
                }
                let feed = shamir_tx::RepoChangefeed::new_with_metadata(
                    Arc::clone(&cl_store),
                    restored,
                    clean_horizon,
                );
                Ok::<ChangefeedHandle, DbError>(ChangefeedHandle {
                    feed,
                    store: cl_store,
                })
            })
            .await?
            .clone();
        // Recheck AFTER OnceCell publishes, not only inside its closure: a failed
        // committer can store pending after the closure snapshot but before
        // get() becomes Some. Together with the publisher's store+get recheck,
        // either that publisher sees the feed or this path sees its minimum.
        // RMW (not just an Acquire load) participates in the pending atomic's
        // modification order. If the publisher's RMW is first, we see its gap;
        // if ours is first, its Acquire observes our Release and therefore the
        // already-published OnceCell. This closes the store-buffering schedule
        // without SeqCst or another lock, including publication after snapshot.
        let pending = self.pending_changefeed_gap.fetch_or(0, Ordering::AcqRel);
        if pending != 0 {
            handle.feed.mark_gap(pending);
        }
        Ok(handle)
    }

    /// Resolve before entering the synchronous publish block. Failure is
    /// logged, but does not mark a gap until this version actually publishes.
    pub(crate) async fn prepare_journal_event(
        &self,
        event: Option<shamir_tx::ChangelogEvent>,
    ) -> Option<(
        Arc<shamir_tx::ChangelogEvent>,
        Option<Arc<shamir_tx::RepoChangefeed>>,
    )> {
        let event = Arc::new(event?);
        let feed = match self.changefeed().await {
            Ok(handle) => Some(handle.feed),
            Err(e) => {
                log::warn!("changefeed init failed for repo {}: {e}", self.name());
                None
            }
        };
        Some((event, feed))
    }

    /// Synchronous publish-time fallback; minimum is shared across repo clones.
    pub(crate) fn record_pending_changefeed_gap(&self, version: u64) {
        let version = version.max(1);
        let _ = self.pending_changefeed_gap.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| {
                Some(if current == 0 {
                    version
                } else {
                    current.min(version)
                })
            },
        );
        if let Some(handle) = self.changefeed.get() {
            handle.feed.mark_gap(version);
        }
    }

    /// Live only: never re-enqueue an event already handled before publish.
    pub(crate) fn broadcast_changefeed_event(&self, event: Option<Arc<shamir_tx::ChangelogEvent>>) {
        if let (Some(event), Some(handle)) = (event, self.changefeed.get()) {
            if handle.feed.subscriber_count() > 0 {
                handle.feed.broadcast(event);
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn fire_post_publish_pre_broadcast_test_hook(&self) {
        if let Some(hook) = self.journal_publish_hook.load_full() {
            if hook.broadcast_armed.swap(false, Ordering::AcqRel) {
                hook.reached.notify_one();
                hook.resume.notified().await;
            }
        }
    }

    /// Subscribe to this repo's live changefeed.
    ///
    /// Returns a `broadcast::Receiver` that yields every `ChangelogEvent`
    /// emitted after the call. A subscriber that falls behind the bounded
    /// ring receives `RecvError::Lagged` and should re-sync the missed
    /// window via [`read_changelog_from`](Self::read_changelog_from).
    pub async fn subscribe_changelog(
        &self,
    ) -> DbResult<tokio::sync::broadcast::Receiver<Arc<shamir_tx::ChangelogEvent>>> {
        Ok(self.changefeed().await?.feed.subscribe())
    }

    /// Resumable pull: read up to `limit` durable journal events with
    /// `commit_version >= from_version`, ascending.
    ///
    /// Returns a [`shamir_tx::JournalRead`] which carries both the events and
    /// a `gap_at` field. When `gap_at` is `Some(v)` the journal has a known
    /// gap at version `v` (a prior overflow dropped that event) — callers
    /// should treat the feed as non-contiguous from `from_version`.
    /// The prefix is bounded by the contiguous completion watermark, not the
    /// potentially out-of-order last committed version. A stalled writer may
    /// return an older (possibly empty) prefix; confirmation logs/counts its
    /// own timeout, so this layer does not log a second warning.
    /// cancel-safe: yes — gate/feed initialization can be retried; confirmation
    /// only advances a monotonic prefix, and range reads mutate no data.
    pub async fn read_changelog_from(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<shamir_tx::JournalRead> {
        Ok(self
            .read_changelog_bounded_impl(from_version, limit, false)
            .await?
            .read)
    }

    /// Read a page and expose the actual confirmed bound, sampled before
    /// feed initialization. Store range failures are errors, not empty pages;
    /// corruption and known holes retain the journal's `gap_at` semantics.
    /// cancel-safe: yes — initialization is retryable, confirmation advances
    /// only a monotonic prefix, and the single range read mutates no records.
    pub async fn read_changelog_bounded(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<super::BoundedJournalRead> {
        self.read_changelog_bounded_impl(from_version, limit, true)
            .await
    }

    async fn read_changelog_bounded_impl(
        &self,
        from_version: u64,
        limit: usize,
        preserve_errors: bool,
    ) -> DbResult<super::BoundedJournalRead> {
        let sampled_w = self.tx_gate().await?.completion().watermark();
        let h = self.changefeed().await?;
        let bound = h
            .feed
            .confirm_through(sampled_w, JOURNAL_CONFIRM_BUDGET)
            .await
            .min(sampled_w);
        let observed = preserve_errors.then(|| {
            Arc::new(super::bounded_journal_read::ErrorPreservingJournalStore::new(h.store.clone()))
        });
        let store: Arc<dyn shamir_tx::ChangelogStore> = match &observed {
            Some(store) => store.clone(),
            None => h.store,
        };
        let read = h
            .feed
            .read_from_bounded(&store, from_version, limit, bound)
            .await;
        if let Some(observed) = observed {
            if let Some(error) = observed.error() {
                return Err(DbError::Storage(error.to_owned()));
            }
        }
        Ok(super::BoundedJournalRead {
            read,
            sampled_w,
            bound,
            confirmed: bound >= sampled_w,
        })
    }

    /// Park until the completion prefix reaches `target_version`, including
    /// aborted/empty versions, or report that this gate requires reopening.
    /// This is completion progress, not journal persistence confirmation.
    /// cancel-safe: yes — retryable gate initialization followed by the
    /// gate's cancellation-safe notification wait; no added polling or state.
    pub async fn await_journal_progress(
        &self,
        target_version: u64,
    ) -> DbResult<shamir_tx::repo_tx_gate::VisibleOutcome> {
        Ok(self.tx_gate().await?.await_visible(target_version).await)
    }

    /// Persisted clean horizon J of the replication journal: every version
    /// through J had its journal event durably checkpointed by a clean stop.
    /// cancel-safe: yes — retryable lazy journal initialization, then an atomic read.
    pub async fn clean_horizon(&self) -> DbResult<u64> {
        Ok(self.changefeed().await?.feed.clean_horizon())
    }

    /// Stop the journal writer so the next clean-horizon checkpoint fails.
    /// Failure-injection seam for shutdown-reporting tests only.
    #[doc(hidden)]
    pub async fn stop_journal_writer_for_test(&self) -> DbResult<()> {
        self.changefeed().await?.feed.shutdown_and_wait().await
    }

    /// Install an isolated journal fixture before lazy initialization.
    #[cfg(test)]
    pub(crate) fn install_journal_for_test(&self, handle: ChangefeedHandle) {
        assert!(self.changefeed.set(handle).is_ok());
    }

    /// Test-only read-side observation of writer persistence/confirmation.
    #[cfg(test)]
    pub(crate) async fn journal_feed_for_test(&self) -> DbResult<Arc<shamir_tx::RepoChangefeed>> {
        Ok(self.changefeed().await?.feed)
    }

    // ============================================================================
    // Index Management API (proxy to TableManager)
    // ============================================================================

    /// cancel-safe: NO — multi-step state mutation. Looks up the table
    /// (one await) then calls `table.create_index` which itself performs
    /// catalogue updates and persistence. Partial cancellation can leave
    /// the index in an inconsistent state; recovery is at the caller's
    /// discretion.
    ///
    /// Create a regular index on a table.
    pub async fn create_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_index(index_name, paths).await
    }

    /// cancel-safe: NO — same shape as `create_index`. Catalogue and
    /// persistence updates inside `table.create_unique_index` are not
    /// atomic across cancellation.
    ///
    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_unique_index(index_name, paths).await
    }

    /// cancel-safe: NO — multi-step state mutation: lookup table then
    /// delete catalogue entries and persisted index data. Partial
    /// cancellation may leave orphaned index state.
    ///
    /// Drop a regular index from a table.
    pub async fn drop_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_index(index_name, None).await
    }

    /// cancel-safe: NO — same shape as `drop_index`. Partial cancellation
    /// may leave orphaned unique-index state.
    ///
    /// Drop a unique index from a table.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_unique_index(index_name, None).await
    }

    /// cancel-safe: yes — read-only: looks up the table and queries
    /// existence. No state mutation; cancellation drops the query.
    ///
    /// Check if a regular index exists on a table.
    pub async fn index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.index_exists(index_name).await)
    }

    /// cancel-safe: yes — read-only existence query. No state mutation.
    ///
    /// Check if a unique index exists on a table.
    pub async fn unique_index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.unique_index_exists(index_name).await)
    }

    /// cancel-safe: yes — read-only lookup. Cancellation drops the query
    /// future with no state mutation.
    ///
    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<shamir_types::types::record_id::RecordId>> {
        let table = self.get_table(table_name).await?;
        table.lookup_by_index(index_name, values).await
    }

    /// cancel-safe: yes — one `scc::HashMap` read plus a single
    /// `get_table` call. Cancellation leaves no state mutated (apart from
    /// the cancel-safe `get_table` OnceCell init).
    ///
    /// Look up the table whose token matches `token`. Used by V2 WAL
    /// recovery to resolve ops by `table_id_interned`, and by the commit
    /// pipeline (Phases 1, 2.6, 5b–5d) while holding the per-repo
    /// `commit_lock`.
    ///
    /// O(1) (III.1): resolves through the `token_names` reverse index that
    /// `add_table` maintains, instead of scanning every config and
    /// re-hashing its name. The old O(N) scan grew the serialized
    /// critical section linearly with schema size. `add_table` and
    /// `remove_table` keep `token_names` in lock-step with `configs`, so a
    /// resolved name always still has a config; `get_table` re-validates
    /// against `configs` inside its init closure regardless.
    pub async fn table_by_token(&self, token: u64) -> DbResult<Option<TableManager>> {
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async`. The
        // `token_names` map is also touched SYNCHRONOUSLY by `register_token`'s
        // `insert_sync` + `read_sync` (DDL — table create/rename) and by
        // drop-table's `remove_if_sync` (DDL). `read_async`'s wait is
        // lock-HANDOFF: saa grants the shared bucket lock to the suspended
        // reader TASK, which then holds it while unpolled in tokio's run queue;
        // a DDL exclusive writer (create/rename/drop) racing sustained
        // commit-pipeline / V2-WAL-recovery `read_async` traffic can park every
        // worker behind that unpolled reader → whole-runtime deadlock.
        // `read_sync`'s bucket lock is held only by a RUNNING thread for a few
        // instructions (a `String` clone), bounding every wait. The fn stays
        // `async`; this call no longer suspends.
        let name = self.token_names.read_sync(&token, |_, name| name.clone());
        match name {
            Some(name) => {
                let tbl = self.get_table(&name).await?;
                Ok(Some(tbl))
            }
            None => Ok(None),
        }
    }

    /// Non-instantiating peek: like [`table_by_token`](Self::table_by_token),
    /// but returns `None` (rather than lazily creating and returning) for a
    /// table that is registered (`add_table` was called) but has never been
    /// touched via [`get_table`](Self::get_table) — the `OnceCell` is still
    /// dormant.
    ///
    /// #538 Part A: `pre_commit_prelock`'s Phase 2.5 uses this instead of
    /// `table_by_token` to check `needs_write_barrier()` for every table in
    /// `tx.write_set`, specifically so that check-only lookup does NOT force
    /// lazy table instantiation as a side effect. This matters because
    /// instantiating a `TableManager` registers it in `per_table_mvcc`,
    /// which flips `apply_data_batch`'s routing for that table's data writes
    /// from the direct-store fallback to MVCC-routed — a behavior change
    /// nothing in `pre_commit_prelock` should trigger merely by CHECKING a
    /// barrier flag. Correctness is preserved: `needs_write_barrier()` can
    /// only ever be `true` on an instance that was actually created — no
    /// code path can flip `index2_create_barrier` or register a base_index
    /// unique index without first holding a live `TableManager` (`get_table`
    /// must have run at least once for a `create_index_v2` /
    /// `create_unique_index` to even be callable) — so a dormant `OnceCell`
    /// can only ever mean "this table has no in-flight barrier", making
    /// `None` here exactly equivalent to "table needs no barrier" for this
    /// check's purposes.
    pub(crate) async fn table_by_token_if_live(&self, token: u64) -> Option<TableManager> {
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async` (same
        // `token_names` map `table_by_token` above also now uses). The map is
        // written EXCLUSIVELY by DDL (`register_token`'s `insert_sync`;
        // drop-table's `remove_if_sync`), and `read_async`'s wait is
        // lock-HANDOFF — a DDL writer racing sustained commit/recovery reads can
        // park every worker behind an unpolled handed-off reader →
        // whole-runtime deadlock. `read_sync`'s bucket lock is held only by a
        // RUNNING thread for a few instructions (a `String` clone), bounding
        // every wait.
        //
        // NON-INSTANTIATING PROPERTY PRESERVED: this change is purely the
        // lock-acquisition mechanism. The lookup still reads the dormant
        // `tables` OnceCell map DIRECTLY (`self.tables.get(&name)`) and does
        // NOT call the lazily-instantiating `get_table` — so the documented
        // reason this fn exists (Phase 2.5's barrier check must NOT force table
        // instantiation as a side effect) is unaffected.
        let name = self.token_names.read_sync(&token, |_, name| name.clone())?;
        let cell = self
            .tables
            .get(&name)
            .map(|entry| Arc::clone(entry.value()))?;
        cell.get().cloned()
    }

    /// cancel-safe: NO — delegates to `recover_inflight_v2` which iterates
    /// entries and replays each one then removes its WAL marker. Mid-
    /// flight cancellation leaves the recovery sequence partially applied;
    /// ops are idempotent so re-invoking is safe.
    ///
    /// Run V2 WAL recovery: replay any inflight tx entries and remove
    /// their markers. Idempotent — safe to call on every open.
    /// Returns the count of recovered entries.
    pub async fn recover_v2_inflight(&self) -> DbResult<usize> {
        crate::tx::recovery::recover_inflight_v2(self).await
    }

    /// Exclusive cold-open certification, before ANY registry insertion.
    /// Conservative, not exact: even a complete journal invalidates (J, H].
    /// Followers at/below H stop with JournalGap/resync_required; automated
    /// reseeding is not supported. A legacy directory without J certifies once.
    /// H is the existing marker/surviving-WAL gate floor, NOT a history maximum.
    pub(crate) async fn certify_cold_start(&self) -> DbResult<()> {
        let gate = self.tx_gate().await?;
        let h = gate.version_allocation_high_water_mark();
        let handle = self.changefeed().await?;
        let j = handle.feed.clean_horizon();
        if h > j {
            // h > j proves j + 1 cannot overflow.
            handle.feed.record_recovery_gap(j + 1, h).await?;
            handle.feed.record_clean_horizon(h).await?;
        }
        Ok(())
    }

    /// Caller MUST stop commit/apply admission before calling this method.
    /// One bounded budget covers quiescence, drain/store flush and checkpoint.
    /// cancel-safe: NO — drain/flush may partially finish; J is never requested
    /// after an earlier failure or timeout. Once admitted, checkpoint I/O can
    /// finish independently; a cancelled caller never reports shutdown success.
    pub async fn shutdown_clean(&self, budget: std::time::Duration) -> DbResult<()> {
        let deadline = tokio::time::Instant::now() + budget;
        self.quiesce_commits(budget).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::timeout(remaining, async {
            self.flush_buffers().await?;
            let gate = self.tx_gate().await?;
            let h = gate.version_allocation_high_water_mark();
            // Settled includes aborted allocations, unlike last_committed.
            // After stopped admission + quiescence + drain, W must equal H;
            // unresolved allocation holes refuse a clean certificate.
            if gate.completion().watermark() != h {
                return Err(DbError::Storage(
                    "unsettled versions at clean shutdown".into(),
                ));
            }
            let handle = self.changefeed().await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(DbError::Storage(
                    "clean shutdown timed out before checkpoint".into(),
                ));
            }
            handle.feed.record_clean_horizon(h).await
        })
        .await
        .map_err(|_| DbError::Storage("clean shutdown timed out".into()))?
    }

    /// Flush this repo's in-memory buffers to their durable backing.
    ///
    /// Drains the `__tx__` store and every table's data + info stores.
    /// In-memory stores are no-ops. Best-effort: individual errors are
    /// logged and skipped; returns the first error encountered (if any)
    /// after attempting all stores.
    pub async fn flush_buffers(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;

        // D2 P1d-2b: drain the inflight WAL tail into `history` before flushing
        // the durable stores. Post-cutover the ack-path writes only the
        // in-memory overlay; the value is durable in `history` only after the
        // drainer replays the WAL entry. A graceful flush therefore drains
        // first so the committed-but-not-yet-drained tail lands durably (and
        // the WAL markers truncate). If no drainer was ever spawned (no commit
        // happened) this is a cheap no-op pass. Best-effort: a drain error is
        // logged and recovery on the next open still converges.
        if let Err(e) = self.drainer().drain_all(self).await {
            log::warn!("flush_buffers: drain_all {}: {}", self.name, e);
            if first_err.is_none() {
                first_err = Some(e);
            }
        }

        match self.tx_info_store().await {
            Ok(store) => {
                if let Err(e) = store.flush().await {
                    log::warn!("flush_buffers: tx_info_store {}: {}", self.name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        for table_name in self.list_table_names() {
            let table = match self.get_table(&table_name).await {
                Ok(t) => t,
                Err(e) => {
                    log::warn!(
                        "flush_buffers: get_table {}/{}: {}",
                        self.name,
                        table_name,
                        e
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                    continue;
                }
            };

            if let Err(e) = table.data_store().flush().await {
                log::warn!(
                    "flush_buffers: data_store {}/{}: {}",
                    self.name,
                    table_name,
                    e
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            if let Err(e) = table.info_store().flush().await {
                log::warn!(
                    "flush_buffers: info_store {}/{}: {}",
                    self.name,
                    table_name,
                    e
                );
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        // Stage I: persist the single per-repo interner ONCE on graceful
        // shutdown. Pre-Stage-I this was a per-table loop; now that every
        // table shares the repo interner, one persist covers all of them
        // (the manager is Arc-shared so any table's `interner()` handle is
        // the same live state). After this, WAL entries whose deltas covered
        // these ids can be safely truncated on next boot.
        match self.repo_interner().await {
            Ok(repo_interner) => {
                if let Err(e) = repo_interner.persist().await {
                    log::warn!("flush_buffers: repo interner persist {}: {}", self.name, e);
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// F6b (I2): flush every per-table `history` version-log to disk WITHOUT
    /// draining the WAL.
    ///
    /// The truncation fsync-gate: the drainer calls this immediately before
    /// `wal.truncate_below(durable)` so that every record in a to-be-deleted
    /// sealed segment is physically durable in `history` before its WAL
    /// segment is unlinked (I2 — closes the power-loss window). Distinct from
    /// [`flush_buffers`](Self::flush_buffers), which drains the WAL first
    /// (`drain_all`) and flushes `data_store`/`info_store` — calling that from
    /// the drainer would recurse. This seam is narrow: it touches ONLY each
    /// MVCC store's `history`, and never the WAL.
    ///
    /// Best-effort over all tables: a per-store flush error is logged and the
    /// first one returned after attempting them all.
    pub async fn flush_all_history(&self) -> DbResult<()> {
        let mut mvccs = Vec::new();
        self.per_table_mvcc.iter_sync(|_, mvcc| {
            mvccs.push(Arc::clone(mvcc));
            true
        });
        let mut first_err: Option<DbError> = None;
        for mvcc in mvccs {
            if let Err(e) = mvcc.flush_history().await {
                log::warn!("flush_all_history: {}: {}", self.name, e);
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Durability flush for a `synced` commit, batched via group-commit so
    /// concurrent synced commits on this repo share one flush+fsync.
    ///
    /// The closure captures a CLONED `RepoInstance` (cheap — every field is
    /// `Arc`-shared) so it is `'static + Send` and can be moved into the
    /// detached leader task `GroupCommit::run` spawns (audit §2.1: the leader
    /// loop must not be tied to the caller's task lifetime, so a cancelled
    /// caller cannot strand `leader_busy`).
    pub async fn synced_flush(&self) -> DbResult<()> {
        let repo = self.clone();
        self.group_commit
            .clone()
            .run(move || {
                let repo = repo.clone();
                async move { repo.flush_buffers().await }
            })
            .await
    }

    /// Force a durable `fsync` of this repo's file WAL (level 2 → level 3).
    /// In-memory repos have no file WAL — this is a no-op there.
    pub async fn sync_wal(&self) -> DbResult<()> {
        self.repo_wal().await?.sync_wal().await
    }

    /// Spawn a background task that runs GC periodically.
    ///
    /// Returns a `tokio::task::JoinHandle` and an `Arc<AtomicBool>` shutdown flag.
    /// Set the flag to `true` to stop the task gracefully.
    ///
    /// The task runs `run_gc()` every `interval`, logging results.
    pub fn spawn_gc_task(
        &self,
        interval: std::time::Duration,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let repo = self.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                match repo.run_gc().await {
                    Ok(0) => {} // nothing to do, stay quiet
                    Ok(n) => log::debug!("GC cleaned {n} history entries"),
                    Err(e) => log::warn!("GC error: {e}"),
                }
            }
        });

        (handle, shutdown)
    }

    /// cancel-safe: NO — multi-table iteration with per-table GC.
    /// Partial cancellation leaves some tables GC'd, others not; the
    /// per-table `gc()` is itself a multi-step scan+delete sequence.
    /// Idempotent on retry (deletes by version threshold).
    ///
    /// Run garbage collection on all tables' history stores.
    ///
    /// Deletes old versions no longer needed by any active snapshot.
    /// Safe to call concurrently with reads/writes — GC only touches
    /// versions below `min_alive`, which no snapshot can read.
    ///
    /// Returns total number of history entries deleted across all tables.
    pub async fn run_gc(&self) -> DbResult<usize> {
        let mut stores: Vec<Arc<shamir_tx::MvccStore>> = Vec::new();
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `iter_sync`, NOT `iter_async`. This is the
        // SAME `per_table_mvcc` map `flush_all_history` (above) already scans
        // with `iter_sync`, and that is also touched by `read_sync`
        // (version_provider.rs, EVERY Serializable `validate_read_set`), `get_sync`
        // (commit.rs pessimistic-lock release; rename-table) and EXCLUSIVELY by
        // `insert_sync` (table attach) / `remove_sync` (drop table). `iter_async`'s
        // wait is lock-HANDOFF: saa grants the bucket lock to the suspended
        // iterator TASK, which then holds it while unpolled in tokio's run queue;
        // a DDL exclusive writer (attach/drop) racing a GC tick can park every
        // worker behind that unpolled iterator → whole-runtime deadlock. `iter_sync`
        // only holds each bucket lock for a RUNNING thread's few instructions (an
        // `Arc::clone` + push), bounding every wait. The fn stays `async` (the
        // per-store `gc().await` below still legitimately suspends); this scan no
        // longer suspends.
        self.per_table_mvcc.iter_sync(|_, mvcc| {
            stores.push(Arc::clone(mvcc));
            true
        });

        let mut total = 0usize;
        for mvcc in stores {
            total += mvcc.gc().await?;
        }

        // Phase C Step 7: prune the per-repo commit-write-log on the SAME
        // GC tick that just pruned per-table `MvccStore::version_cache`.
        // Uses the gate's `min_alive()` — byte-for-byte the same threshold
        // the per-store `MvccStore::gc()` consumed inside
        // `prune_version_cache` (mvcc_store.rs:447). Identical discipline,
        // identical safety argument (see `RepoTxGate::prune_commit_log_below`
        // doc-comment and the invariant on mvcc_store.rs:416-441).
        //
        // Zero-overhead on non-Serializable repos: the log is empty, so
        // `prune_commit_log_below` walks an empty `TreeIndex` range and
        // returns 0 immediately. The gate may not even exist yet (the
        // `OnceCell` is lazy-init'd on the first `tx_gate()` call); we use
        // `self.tx_gate.get()` (non-allocating peek) to skip cleanly when
        // the gate has never been initialised.
        //
        // Lock order: `prune_commit_log_below` takes NO locks — it uses
        // `scc::TreeIndex::remove_range` (lock-free CAS). Callers MUST NOT
        // hold `commit_mutex` here (we don't — `run_gc` is an independent
        // background task).
        if let Some(gate) = self.tx_gate.get() {
            let min_alive = gate.min_alive();
            let _pruned = gate.prune_commit_log_below(min_alive);
        }

        self.tx_metrics.on_gc_run(total);
        Ok(total)
    }
}

/// Deterministic u64 token for a repository name.
///
/// Stage 4: `DefaultHasher(name)` placeholder.
/// Stage 5: real repo-level interner ID.
pub fn repo_token(name: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

/// Record `table_token_for(name) → name` in the reverse index, detecting
/// the two non-trivial cases explicitly:
///
/// * **Idempotent re-registration** — the same name added twice (e.g.
///   `create_table` issued twice, or catalogue reload over an existing
///   config). The existing entry already equals `name`, so this is a
///   no-op.
/// * **Token collision** — two *distinct* names that hash to the same
///   `u64` token. With a 64-bit `DefaultHasher` over table names this is
///   astronomically unlikely, but if it ever happens we keep the FIRST
///   registration (do not clobber a live mapping) and log a warning so the
///   situation is visible instead of silently corrupting `table_by_token`
///   for the first table. The second table is then unresolvable by token —
///   the caller should rename it.
fn register_token(token_names: &scc::HashMap<u64, String, THasher>, name: &str) {
    let token = table_token_for(name);
    if let Err((_, attempted)) = token_names.insert_sync(token, name.to_string()) {
        // Key already present — inspect the existing mapping.
        let existing = token_names.read_sync(&token, |_, n| n.clone());
        if existing.as_deref() != Some(attempted.as_str()) {
            log::warn!(
                "repo_instance: table token collision on {} — keeping '{}', \
                 refusing '{}'; the latter is unresolvable by token",
                token,
                existing.as_deref().unwrap_or("<unknown>"),
                attempted
            );
        }
    }
}

/// Convert the wire DTO [`shamir_query_types::admin::Retention`] into the
/// engine-internal [`shamir_tx::Retention`]. The orphan rule forbids a
/// cross-crate `From` impl, so this free fn bridges the two types.
///
/// Call `Retention::validate()` on the DTO first; this fn performs no
/// validation (it is a pure field copy).
pub fn to_mvcc_retention(r: &shamir_query_types::admin::Retention) -> shamir_tx::Retention {
    shamir_tx::Retention {
        max_age_secs: r.max_age_secs,
        max_count: r.max_count,
        min_count: r.min_count,
    }
}
