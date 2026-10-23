use crate::repo::changelog_store::write_domain::RepoWriteDomain;
use crate::repo::durability_policy::DurabilityPolicy;
use crate::repo::fk_reverse_cache::FkReverseCache;
use crate::repo::repo_types::{BoxRepoFactory, RepoFactory};
use crate::table::interner_manager::InternerManager;
use crate::table::TableConfig;
use crate::tx::CommitTailBarrier;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{Repo, Store};
use shamir_types::types::common::THasher;
use std::sync::Arc;

use super::RepoInstance;

mod clean_horizon_repo_impl;
mod durability_repo_impl;
mod recovery;
mod seed_backing_impl;
mod seed_metadata_impl;

impl RepoInstance {
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
    /// Identical to a foreground [`Clone`] except `live = None` (the task
    /// does not count toward repo liveness) and `worker_lifecycle = None`
    /// (the task does not retain foreground lifecycle ownership).
    /// See [`Drainer::spawn`](crate::tx::drainer::Drainer::spawn).
    fn clone_for_background(&self) -> Self {
        let mut bg = self.clone();
        bg.live = None;
        // Background handles own only drainer resources, never foreground
        // lifecycle ownership.
        bg.worker_lifecycle = None;
        bg
    }

    /// Read-only progress of initialized state; never opens stores or starts
    /// a drainer. An untouched gate reports an idle snapshot.
    pub fn diagnostics(&self) -> shamir_tx::GateDiagnostics {
        let mut snapshot = self
            .tx_gate
            .get()
            .map(|gate| gate.diagnostics())
            .unwrap_or_default();
        if let Some(drainer) = self.drainer.get() {
            drainer.enrich_diagnostics(&mut snapshot);
        }
        snapshot
    }

    /// D2 P1d-2b: the repo's background drainer, spawned once (single-owner).
    ///
    /// First call lazily creates the [`Drainer`](crate::tx::drainer::Drainer)
    /// and starts its background loop (a leak-free task owning a
    /// `clone_for_background` of this repo plus a
    /// `Weak<()>` of `live`). Subsequent calls (from any
    /// foreground clone) return the same `Arc<Drainer>` via the shared
    /// `OnceCell`. The commit path calls `repo.drainer().wake()` after
    /// publishing a version to nudge the loop.
    pub fn drainer(&self) -> Arc<crate::tx::drainer::Drainer> {
        #[cfg(test)]
        if self
            .background_drainer_disabled
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Arc::clone(
                self.drainer
                    .get_or_init(|| Arc::new(crate::tx::drainer::Drainer::new())),
            );
        }

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
        Self::from_factory_with_durability(name, factory, configs, DurabilityPolicy::default())
            .await
    }

    /// Unrecovered disk handle solely for tests that explicitly exercise replay
    /// or must install a fault before recovery. Not available in production.
    #[cfg(all(test, feature = "fjall"))]
    pub(crate) async fn from_factory_for_recovery_test(
        name: String,
        factory: BoxRepoFactory,
        configs: Vec<TableConfig>,
    ) -> DbResult<Self> {
        Self::from_factory_unrecovered(name, factory, configs).await
    }

    pub(in crate::repo::instance::repo_instance) async fn from_factory_unrecovered(
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

    /// S5c-X PR6b-2a: this repository's write-admission domain (one per
    /// repository instance, shared by every clone).
    pub fn write_domain(&self) -> &Arc<RepoWriteDomain> {
        &self.write_domain
    }

    /// Repository name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return a clone of this `RepoInstance` with its `name` field replaced
    /// by `new_name`. Used by `DbInstance::rename_repo` (Phase F.3) to
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
    /// cancel-safe: NO — delegates to `recover_inflight_v2` which iterates
    /// entries and replays each one then removes its WAL marker. Mid-
    /// flight cancellation leaves the recovery sequence partially applied;
    /// ops are idempotent so re-invoking is safe.
    ///
    /// Run V2 WAL recovery: replay any inflight tx entries and remove
    /// their markers. Idempotent — safe to call on every open.
    /// Returns the count of recovered entries.
    pub async fn recover_v2_inflight(&self) -> DbResult<usize> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        // Round-11 (stack layout): heap-pin the recovery body (replays every
        // inflight entry through the admitted chain) off this wrapper's frame.
        Box::pin(crate::tx::recovery::recover_inflight_v2(self, &permit)).await
    }

    pub(crate) async fn recover_v2_inflight_admitted(
        &self,
        permit: &crate::repo::RepoWritePermit,
    ) -> DbResult<usize> {
        self.write_domain.debug_check_permit(permit);
        // Round-11 (stack layout): see `recover_v2_inflight`.
        Box::pin(crate::tx::recovery::recover_inflight_v2(self, permit)).await
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
        let wal = self.repo_wal().await?;
        if self.initialized_mem_wal() {
            return Ok(());
        }
        wal.sync_wal().await
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
