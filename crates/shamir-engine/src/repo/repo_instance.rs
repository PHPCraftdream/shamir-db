use super::super::table::{TableConfig, TableManager};
use super::group_commit::GroupCommit;
use super::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use crate::query::batch::BatchError;
use crate::query::write::WriteResult;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{Repo, Store};
use shamir_types::access::Actor;
use shamir_types::types::common::{new_dash_map_wc, TDashMap, THasher};
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::table::table_manager::table_token_for;

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
    /// Group-commit coordinator for `synced` durability flushes.
    /// Shared across all clones so concurrent synced commits on this
    /// repo batch their flush+fsync into a single I/O round.
    group_commit: Arc<GroupCommit>,
    /// Lazily-initialised per-repo changefeed (Phase 3b): live broadcast +
    /// durable journal writer over the `"__changelog__"` store. Created on
    /// first call to [`changefeed`](Self::changefeed). Shared across clones.
    changefeed: Arc<OnceCell<ChangefeedHandle>>,
    /// On-disk backing directory for this repo, captured from the factory's
    /// `backing_dir()` at construction. `Some(dir)` for disk-backed repos —
    /// [`repo_wal`](Self::repo_wal) builds a file-backed `WalGroupCommit`
    /// rooted at `dir/wal`. `None` for in-memory/test repos — the WAL falls
    /// back to KV markers. INERT plumbing (W3): the file WAL is constructed
    /// but not yet written by the live commit path (W4/W5).
    wal_dir: Option<std::path::PathBuf>,
}

/// Bundle of the per-repo changefeed and the store it journals into.
/// The store is retained so [`RepoInstance::read_changelog_from`] can
/// range-read the journal without re-resolving it.
#[derive(Clone)]
struct ChangefeedHandle {
    feed: Arc<shamir_tx::RepoChangefeed>,
    store: Arc<dyn shamir_tx::ChangelogStore>,
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
            group_commit: Arc::clone(&self.group_commit),
            changefeed: Arc::clone(&self.changefeed),
            wal_dir: self.wal_dir.clone(),
        }
    }
}

impl RepoInstance {
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
            group_commit: Arc::new(GroupCommit::new()),
            changefeed: Arc::new(OnceCell::new()),
            wal_dir,
        }
    }

    /// cancel-safe: yes — single `factory.create().await`; cancellation
    /// before completion drops any half-constructed repo with no
    /// externally observable state change.
    ///
    /// Creates a RepoInstance asynchronously from a factory.
    /// This is the preferred method as it properly handles blocking I/O.
    pub async fn from_factory(
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

    /// Per-table MvccStore map used by the commit pipeline to route
    /// data writes through version-aware storage.
    pub fn per_table_mvcc(&self) -> &Arc<scc::HashMap<u64, Arc<shamir_tx::MvccStore>, THasher>> {
        &self.per_table_mvcc
    }
    /// Atomic transaction telemetry counters.
    pub fn tx_metrics(&self) -> &Arc<shamir_tx::TxMetrics> {
        &self.tx_metrics
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
        cell.get_or_try_init(|| async move {
            if !self.configs.contains_key(table_name) {
                return Err(DbError::NotFound(format!(
                    "Table '{}' is not configured in this repository",
                    table_name
                )));
            }
            self.create_table_context(table_name).await
        })
        .await
        .cloned()
    }

    async fn create_table_context(&self, table_name: &str) -> DbResult<TableManager> {
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
        let mvcc = Arc::new(shamir_tx::MvccStore::new(history_store, Arc::clone(&gate)));

        let token = table_token_for(table_name);
        let _ = self.per_table_mvcc.insert(token, Arc::clone(&mvcc));

        let tbl = TableManager::create(table_name.to_string(), data_store, info_store).await?;
        let tbl = tbl.with_mvcc_store(mvcc);

        // Changefeed (Phase 3b follow-up): wire the non-tx write path to the
        // per-repo changefeed so non-transactional insert/update/set/delete
        // emit `ChangelogEvent`s too — not just the tx commit pipeline. The
        // table is handed the SAME `gate` the commit pipeline uses, so non-tx
        // and tx `commit_version`s share one monotonic sequence per repo. The
        // feed is "always on" by design, so resolving it here (eagerly, once
        // per table) is consistent with that contract; an init failure is
        // logged and the table simply runs without non-tx changefeed (the tx
        // path still emits via its own lazy resolution).
        match self.changefeed().await {
            Ok(h) => Ok(tbl.with_changefeed(self.name.clone(), Arc::clone(&gate), h.feed)),
            Err(e) => {
                log::warn!(
                    "create_table_context: changefeed unavailable for repo {} table {}: {e}; \
                     non-tx writes on this table will not emit changefeed events",
                    self.name,
                    table_name
                );
                Ok(tbl)
            }
        }
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.iter().map(|e| e.key().clone()).collect()
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
    pub fn add_table(&self, config: TableConfig) {
        register_token(&self.token_names, &config.name);
        self.configs.insert(config.name.clone(), config);
    }

    /// Remove a table from the repository.
    /// Returns true if the table existed and was removed.
    pub fn remove_table(&self, table_name: &str) -> bool {
        let removed = self.configs.remove(table_name).is_some();
        if removed {
            self.tables.remove(table_name);
            // Drop the reverse-index entry only if it still points at the
            // table we removed. A (astronomically unlikely) token collision
            // could have left another name's mapping in place; never evict
            // someone else's mapping.
            let token = table_token_for(table_name);
            let _ = self
                .token_names
                .remove_if(&token, |existing| existing.as_str() == table_name);
        }
        removed
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
    /// `list_inflight()` recovery runs and happens once per gate (OnceCell).
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

    /// cancel-safe: yes — read-only `list_inflight()` scan over the
    /// repo's `"__tx__"` store; cancellation drops the future with no
    /// state change.
    ///
    /// Returns the current (last committed) version for this repo.
    ///
    /// This is the highest `commit_version` that has been fully committed
    /// and published. Useful for seeding subscription watermarks.
    pub async fn current_commit_version(&self) -> DbResult<u64> {
        let gate = self.tx_gate().await?;
        Ok(gate.last_committed())
    }

    /// Highest `commit_version` across all durable inflight V2 WAL
    /// entries, or `0` if there are none. Used to seed the tx gate's
    /// version floor (CRIT-B) so recovered commit versions are never
    /// re-issued.
    pub(crate) async fn max_inflight_commit_version(&self) -> DbResult<u64> {
        let wal = self.repo_wal().await?;
        let entries = wal.list_inflight().await?;
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
    /// `max(persisted_NextTxId, max_inflight_txn_id + 1)`.
    ///
    /// **Context (KV-marker model only).** In KV-backed repos each WAL
    /// entry is stored under `WalActiveKey(txn_id)` — a unique marker.
    /// If two entries shared a `txn_id` the second would overwrite the
    /// first marker, silently losing inflight state. The persisted
    /// `NextTxId` snapshot is written only periodically, so after a
    /// crash it can lag behind an inflight entry's txn_id. Flooring the
    /// counter above the max inflight txn_id prevents reuse.
    ///
    /// **File mode (append-only segment).** The live tx commit path
    /// stamps WAL entries with the gate's `tx.tx_id.0` (see
    /// `pre_commit.rs`), NOT `RepoWalManager::fresh_txn_id()` — the
    /// latter has no caller in the tx commit path. Furthermore, in file
    /// mode `list_inflight()` returns empty (no KV markers are written),
    /// so the floor below is a no-op. Even if txn_ids repeated after a
    /// crash, the segment is append-only: entries are sequential frames
    /// keyed by position, and recovery replays by the monotonically-
    /// floored `commit_version` — no ordering ambiguity or data loss.
    ///
    /// The floor is therefore a **KV-path-only safeguard**, inert in
    /// file mode by construction.
    pub async fn repo_wal(&self) -> DbResult<Arc<shamir_tx::RepoWalManager>> {
        self.repo_wal
            .get_or_try_init(|| async {
                let info_store = self.repo.store_get("__tx__").await?;
                let initial_txn_id =
                    crate::meta::recovery_marker::load_next_tx_id_snapshot(&info_store)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(1);
                // F2: build the file-backed `WalSink` + `WalGroupCommit` for
                // disk repos (sibling `<name>.shamirwal/` directory via
                // `file_name()` — NOT `OsString::push` on the full path, which
                // breaks on trailing separators), or `Noop` for in-memory
                // repos. F3: the live commit path now appends via
                // `begin_grouped(Buffered)` and recovery replays via
                // `recover()` (segment `replay()` in file mode).
                // F3: only disk repos get a file-backed group (which drives
                // `begin_grouped`/`recover` over the segment). In-memory repos
                // (no `wal_dir`) keep the KV-marker path via `new` — durability
                // is meaningless for RAM-only repos, and the no-group path is
                // what `commit`/`recover` fall back to. Tying the group to a
                // File sink (never Noop) makes `group.is_some()` ⟺ file mode.
                let mgr = match &self.wal_dir {
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
                        let seg = shamir_wal::WalSegment::open(wal_dir.join("repo.wal")).await?;
                        let sink = shamir_wal::WalSink::File(seg);
                        let group = Arc::new(shamir_wal::WalGroupCommit::new(Arc::new(sink)));
                        // RF1: background fsync bounds the power-loss window for
                        // Buffered (level 2) commits. 250 ms = max data-at-risk.
                        const WAL_BG_FSYNC_MS: u64 = 250;
                        group.spawn_background_fsync(std::time::Duration::from_millis(
                            WAL_BG_FSYNC_MS,
                        ));
                        shamir_tx::RepoWalManager::new_with_group(info_store, initial_txn_id, group)
                    }
                    None => shamir_tx::RepoWalManager::new(info_store, initial_txn_id),
                };

                // KV-path safeguard: floor next_txn_id above any inflight
                // txn_id to prevent marker key collision after crash.
                // Inert in file mode — list_inflight() returns empty (no
                // KV markers written), so the branch is never taken.
                if let Ok(entries) = mgr.list_inflight().await {
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

        if isolation == shamir_tx::IsolationLevel::Serializable {
            let provider = std::sync::Arc::new(crate::repo::RepoVersionProvider {
                per_table_mvcc: Arc::clone(&self.per_table_mvcc),
            });
            tx.set_version_provider(provider);
        }

        Ok((tx, guard))
    }

    /// cancel-safe: partial — delegates to [`crate::tx::commit_tx`], whose
    /// commit point is a *successful* Phase 4 `wal.begin` (the durable WAL
    /// entry IS the commit), not the completion of the whole pipeline. A
    /// cancellation BEFORE that point is a clean abort — nothing durable.
    /// A cancellation AT/AFTER it leaves the tx COMMITTED: the inflight WAL
    /// marker is replayed idempotently by recovery on the next open, which
    /// reconciles materialization (I.3). See `commit_tx` in `tx/commit.rs`
    /// for the full rationale.
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
        let (mut tx, _guard) = self
            .begin_tx(shamir_tx::IsolationLevel::Snapshot)
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

        // Stage the write into the tx. On error drop tx/_guard = RAII abort.
        let wr = stage(&mut tx).await.map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: e.to_string(),
            code: None,
        })?;

        // Commit — folds everything into one WalEntryV2 / one commit_version.
        match self.commit_tx(tx).await {
            Ok(_outcome) => Ok(wr),
            Err(commit_err) => {
                let (message, code) = match commit_err {
                    crate::tx::CommitError::UniqueViolation { .. } => {
                        (commit_err.to_string(), Some("unique_violation".to_string()))
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
    async fn changefeed(&self) -> DbResult<ChangefeedHandle> {
        self.changefeed
            .get_or_try_init(|| async {
                let store: Arc<dyn Store> = self.repo.store_get("__changelog__").await?;
                let cl_store: Arc<dyn shamir_tx::ChangelogStore> =
                    Arc::new(crate::repo::changelog_store::StoreChangelog::new(store));
                let feed = shamir_tx::RepoChangefeed::new(Arc::clone(&cl_store));
                Ok::<ChangefeedHandle, DbError>(ChangefeedHandle {
                    feed,
                    store: cl_store,
                })
            })
            .await
            .cloned()
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
    pub async fn read_changelog_from(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<shamir_tx::JournalRead> {
        let h = self.changefeed().await?;
        Ok(h.feed.read_from(&h.store, from_version, limit).await)
    }

    /// cancel-safe: yes — hands a pre-projected event to the changefeed's
    /// two non-blocking tracks (broadcast `send` + journal `try_send`).
    /// NEVER blocks the commit-path and NEVER errors out to it.
    ///
    /// Emit a committed tx's projected footprint to the changefeed. The
    /// event must be projected by the caller (via [`shamir_tx::project_event`])
    /// BEFORE Phase 5a drains `tx.write_set`, and emitted AFTER
    /// `gate.publish_committed` so subscribers/journal readers never observe
    /// a version the gate has not yet published. `None` (an empty-footprint
    /// commit) is a no-op.
    pub(crate) async fn emit_changefeed_event(&self, event: Option<shamir_tx::ChangelogEvent>) {
        let Some(event) = event else {
            return; // empty footprint — nothing to emit
        };
        let commit_version = event.commit_version;
        match self.changefeed().await {
            Ok(h) => h.feed.emit(event),
            Err(e) => {
                // Changefeed init failed (e.g. store_get error). The commit
                // is already durable; the feed is best-effort, so log + move
                // on rather than fail the commit.
                log::warn!(
                    "emit_changefeed: changefeed unavailable for repo {} commit_version \
                     {commit_version}: {e}; skipping changefeed emission",
                    self.name()
                );
            }
        }
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
        table.drop_index(index_name).await
    }

    /// cancel-safe: NO — same shape as `drop_index`. Partial cancellation
    /// may leave orphaned unique-index state.
    ///
    /// Drop a unique index from a table.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_unique_index(index_name).await
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
        let name = self
            .token_names
            .read_async(&token, |_, name| name.clone())
            .await;
        match name {
            Some(name) => {
                let tbl = self.get_table(&name).await?;
                Ok(Some(tbl))
            }
            None => Ok(None),
        }
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

    /// Flush this repo's in-memory buffers to their durable backing.
    ///
    /// Drains the `__tx__` store and every table's data + info stores.
    /// In-memory stores are no-ops. Best-effort: individual errors are
    /// logged and skipped; returns the first error encountered (if any)
    /// after attempting all stores.
    pub async fn flush_buffers(&self) -> DbResult<()> {
        let mut first_err: Option<DbError> = None;

        if let Ok(store) = self.tx_info_store().await {
            if let Err(e) = store.flush().await {
                log::warn!("flush_buffers: tx_info_store {}: {}", self.name, e);
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
            // A5: persist each table's interner on graceful shutdown so
            // all in-memory (name, id) mappings are durable before the
            // process exits. After this, WAL entries whose deltas covered
            // these ids can be safely truncated on next boot.
            if let Err(e) = table.interner().persist().await {
                log::warn!(
                    "flush_buffers: interner persist {}/{}: {}",
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

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Durability flush for a `synced` commit, batched via group-commit so
    /// concurrent synced commits on this repo share one flush+fsync.
    pub async fn synced_flush(&self) -> DbResult<()> {
        self.group_commit.run(|| self.flush_buffers()).await
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
        self.per_table_mvcc
            .scan_async(|_, mvcc| stores.push(Arc::clone(mvcc)))
            .await;

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
    use std::hash::{Hash, Hasher};
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
    if let Err((_, attempted)) = token_names.insert(token, name.to_string()) {
        // Key already present — inspect the existing mapping.
        let existing = token_names.read(&token, |_, n| n.clone());
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
