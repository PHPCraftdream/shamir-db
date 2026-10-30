//! S5c-X — repository write-domain admission plumbing for the TABLE layer.
//!
//! `#[path]`-wired impl-block file (a CHILD of the `table_manager` module,
//! declared at the top of `table_manager.rs`): being a descendant module, it
//! can see `TableManager`'s private/`pub(in crate::table)` fields, which is what makes
//! the move out of the over-limit `table_manager.rs` legal without widening
//! any visibility. See `repo/journal/changelog_store.rs` for the same pattern.
//!
//! Contents:
//! - [`TableManager::admit`] — per-repo admission acquisition (lock order root).
//! - [`TableManager::begin_write_barrier_admitted`] — INFALLIBLE admitted twin
//!   of `begin_write_barrier` (its body only debug-checks the permit and
//!   locks — nothing can fail).
//! - [`TableManager::begin_write_barrier_with_permit`] — public, DbResult
//!   future-proofing wrapper for cross-crate admitted callers.
//! - [`TableManager::create_with_domain_admitted`] — the whole open/create
//!   body, admitted; `create_with_domain` admits once and delegates.

use std::sync::Arc;

use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::SortedIndexManager;
use crate::repo::{RepoWriteDomain, RepoWritePermit};
use crate::table::buffer_config;
use crate::table::table::Table;
use crate::table::{InternerManager, PersistRegistry, RecordCounter};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use super::TableManager;
use super::WriteBarrierGuard;

impl TableManager {
    /// S5c-X — acquire per-REPOSITORY write admission. Lock order (BINDING):
    /// repo permit FIRST → per-table `ddl_admission` → raise-intent →
    /// drain-writers → `unique_write_lock` → terminal
    /// `bindings_lock`/`persist_lock`. The leader write/commit paths never
    /// call this — only DDL and registry-flush paths pay admission.
    pub(in crate::table) async fn admit(&self) -> DbResult<RepoWritePermit> {
        self.write_domain
            .admit()
            .await
            .map_err(|e| DbError::Internal(format!("repository write admission failed: {e}")))
    }
    /// S5c-X — admitted twin of [`begin_write_barrier`](Self::begin_write_barrier):
    /// the caller ALREADY holds repo admission (`permit`); this verifies the
    /// permit belongs to this repository's domain, then runs the existing
    /// sequence unchanged. The caller keeps ownership of `permit` (it is
    /// only borrowed here for the debug identity check), so repo admission
    /// spans the whole surrounding critical section.
    pub async fn begin_write_barrier_admitted(
        &self,
        bit: u8,
        permit: &RepoWritePermit,
    ) -> (WriteBarrierGuard, tokio::sync::OwnedMutexGuard<()>) {
        self.write_domain.debug_check_permit(permit);
        self.begin_write_barrier_admitted_impl(bit).await
    }

    pub(in crate::table) async fn begin_write_barrier_admitted_impl(
        &self,
        bit: u8,
    ) -> (WriteBarrierGuard, tokio::sync::OwnedMutexGuard<()>) {
        // Step 0 (F-95, #957): acquire the per-table DDL admission mutex
        // FIRST. This serializes concurrent DDL operations that would
        // share the same bit: without it, DDL-B can drain and park on
        // `unique_write_lock` BEFORE DDL-A drops its WriteBarrierGuard
        // (which unconditionally clears the bit). When B later acquires
        // the lock, the bit is 0 and B never re-raises it — a new writer
        // then fast-paths past B's in-progress backfill. The admission
        // guard is embedded inside the returned `WriteBarrierGuard` so it
        // is held for the caller's ENTIRE critical section (not just this
        // call), and dropped AFTER the bit is cleared (field-drop order:
        // the manual `Drop::drop` clears the bit first, then the
        // `admission` field is dropped, releasing the mutex).
        let admission = self.ddl_admission.clone().lock_owned().await;
        // Step 1: raise the intent bit FIRST — new writers now take the slow
        // (locked) path.
        let guard = WriteBarrierGuard::set(self.write_barrier_flags.clone(), bit, admission, None);
        // Step 2: drain every writer that read the flag as `false` BEFORE
        // step 1 and is still mid-flight. Cheap (one SeqCst load) when none
        // are active.
        self.drain_writers().await;
        // Step 3: ONLY NOW acquire the lock — see the module doc for why
        // this order (not the reverse) is what closes the F-70 cycle.
        let lock_guard = self.unique_write_lock.clone().lock_owned().await;
        (guard, lock_guard)
    }

    /// S5c-X — public admitted barrier entry for CROSS-CRATE callers
    /// (e.g. `shamir-db`'s schema-activation DDL handler) that already hold
    /// a `RepoWritePermit` minted by the repository: runs the admitted
    /// sequence WITHOUT admitting again (the caller keeps ownership of
    /// `permit`; the returned guard does NOT retain it). The `DbResult`
    /// wrapper is kept deliberately: the admitted path is infallible today,
    /// but the cross-crate signature must stay future-proof without
    /// re-minting admission.
    pub async fn begin_write_barrier_with_permit(
        &self,
        bit: u8,
        permit: &RepoWritePermit,
    ) -> DbResult<(WriteBarrierGuard, tokio::sync::OwnedMutexGuard<()>)> {
        self.write_domain.debug_check_permit(permit);
        let (guard, lock) = self.begin_write_barrier_admitted_impl(bit).await;
        Ok((guard, lock))
    }

    /// S5c-X ADMITTED twin of [`TableManager::create_with_domain`] — the
    /// whole open/create body lives here exactly once; the public wrapper
    /// (in `table_manager.rs`) admits once and delegates. The caller already
    /// holds repo admission (`permit`, debug-checked against this table's
    /// domain); the open-path recovery steps reuse that permit instead of
    /// re-admitting — the domain mutex is non-reentrant, a second admit here
    /// would self-deadlock.
    pub async fn create_with_domain_admitted(
        name: String,
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
        write_domain: Arc<RepoWriteDomain>,
        permit: &RepoWritePermit,
    ) -> DbResult<Self> {
        write_domain.debug_check_permit(permit);
        let interner = InternerManager::new(Arc::clone(&info_store));
        let counter = Arc::new(RecordCounter::new_with_domain(
            Arc::clone(&info_store),
            Arc::clone(&write_domain),
        ));

        // Build the persist registry — cloning interner shares all its
        // internal Arcs (same underlying data), wrapping in Arc<dyn Persistable>
        // gives the uniform flush surface.
        let mut persist_registry = PersistRegistry::new();
        persist_registry
            .register(Arc::new(interner.clone()) as Arc<dyn crate::table::persistable::Persistable>);
        persist_registry
            .register(Arc::clone(&counter) as Arc<dyn crate::table::persistable::Persistable>);

        // #1048: read the hash DROP tombstones BEFORE IndexManager::new clears them,
        // so we can write SucceededViaCrashRecovery status using the interner to resolve
        // name_interned back to string names.
        let dropping_regular_before =
            shamir_index::base_index::index_manager::IndexManager::load_dropping_set_standalone(
                false,
                &info_store,
            )
            .await
            .unwrap_or_default();
        let dropping_unique_before =
            shamir_index::base_index::index_manager::IndexManager::load_dropping_set_standalone(
                true,
                &info_store,
            )
            .await
            .unwrap_or_default();

        let index_manager =
            IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store)).await?;
        // F-69 (#896): clone the SAME packed word `index_manager` uses for its
        // `UNIQUE_INDEX_EXISTS` bit — `TableManager` folds its own five
        // DDL-intent bits into this identical `Arc<AtomicU8>`, below.
        let write_barrier_flags = index_manager.write_barrier_flags();
        let sorted_indexes = SortedIndexManager::new(Arc::clone(&info_store)).await?;
        let table = Table::new(data_store);

        // #1048 / #1051: write SucceededViaCrashRecovery status for any hash
        // DROP operations that were recovered. Use the interner to resolve
        // name_interned back to string names; the op_id itself is read
        // directly off the tombstone entry (minted with real entropy at
        // dispatch time — no longer regenerated deterministically).
        if !dropping_regular_before.is_empty() || !dropping_unique_before.is_empty() {
            TableManager::write_hash_drop_recovery_status(
                &dropping_regular_before,
                &dropping_unique_before,
                &interner,
                Arc::clone(&info_store),
            ).await
            .map_err(|e| {
                log::error!("#1048: failed to write SucceededViaCrashRecovery status for recovered hash DROP operations: {e}");
                shamir_storage::error::DbError::Internal(format!(
                    "#1048: failed to write SucceededViaCrashRecovery status for recovered hash DROP operations: {e}"
                ))
            })?;
        }

        // Pre-load validator bindings from the info-twin (S2).
        let validator_bindings =
            match crate::validator::persistence::load_validators_metadata(&info_store).await? {
                Some(pv) => Arc::new(arc_swap::ArcSwap::from_pointee(pv.bindings)),
                None => Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())),
            };

        let mgr = Self {
            token: super::table_token_for(&name),
            name,
            table: Arc::new(table),
            info_store: Arc::clone(&info_store),
            interner,
            counter,
            persist_registry,
            index_manager,
            sorted_indexes,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unique_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_barrier_flags,
            // F-95 (#957) — fresh per-table admission mutex.
            ddl_admission: Arc::new(tokio::sync::Mutex::new(())),
            // S5c-X — shared per-repo write-admission domain.
            write_domain: Arc::clone(&write_domain),
            writer_drain: crate::table::writer_drain_barrier::WriterDrainBarrier::new(),
            in_flight_creates: crate::table::in_flight_create_guard::InFlightCreateSet::new(),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            #[cfg(test)]
            filtered_ann_thresholds: None,
            mvcc_store: None,
            validator_bindings,
            bindings_lock: Arc::new(tokio::sync::Mutex::new(())),
            validator_registry: None,
            changefeed: None,
            scalar_resolver: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                shamir_funclib::scalar_resolver::ScalarResolver::builtins_only(),
            ))),
            #[cfg(test)]
            create_index2_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            create_sorted_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            drop_index2_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            drop_index2_post_sweep_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            rename_index2_early_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            rename_index2_mid_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            index2_registered_before_ready_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            online_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_b_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_c_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            phase_d_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            #[cfg(test)]
            verify_panic_hook: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            verify_call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        // Resolve covering-index included_fields string paths to interned ids.
        // The sorted_indexes manager loads definitions from disk with
        // `included_fields_interned = []` (serde skip), so we rebuild the
        // transient cache here, right after open, before any write path runs.
        //
        // Skip if no sorted indexes exist — avoids forcing early interner
        // initialization before `with_interner` can replace it with the
        // shared per-repo manager.
        if mgr.sorted_indexes.has_covering_indexes() {
            if let Ok(interner) = mgr.interner.get().await {
                mgr.sorted_indexes.intern_included_paths(interner);
            }
        }

        // Hot-load persisted buffer config (if any) and apply
        // it to both stores. If no DDL has set one, the stores
        // keep whatever default the factory wrapped them with.
        if let Some(cfg) = buffer_config::load(&mgr.info_store).await? {
            mgr.table.data_store().apply_buffer_config(&cfg).await?;
            mgr.info_store.apply_buffer_config(&cfg).await?;
        }

        // Restore index2 backends from persisted metadata.
        //
        // F-50 Step 3b — self-healing restart-from-scratch: a descriptor
        // loaded with `state == Building` marks an index whose backfill was
        // interrupted by a crash between `create_index_v2`'s first
        // (`Building`) and final (`Ready`) persist. The half-built backend's
        // partial postings are safely droppable (the planner Ready-gate kept
        // every reader off them), so we drop them, re-run the backfill from
        // scratch, flip the state to `Ready`, and re-persist — making the
        // recovery fully automatic, no operator action needed. See the
        // decision memo (`docs/dev-artifacts/research/f50-step3-crash-restart-spike.md`
        // §2) for why restart-from-scratch was chosen over resume.
        let mut recovered_building_ids: Vec<u32> = Vec::new();
        // R0-D (#1013): ids that failed the Building self-heal (a failed
        // `drop_all`) and were marked `Failed` in the loop below. Threaded
        // through to the `restore_on_open` loop so it does not redundantly
        // (and pointlessly) attempt to restore a backend already known to
        // be broken.
        let mut failed_recovery_ids: Vec<u32> = Vec::new();
        if let Some(persisted) =
            crate::index2::persistence::load_index2_metadata(&mgr.info_store).await?
        {
            mgr.index2_registry.set_next_id(persisted.next_id);
            for desc in persisted.descriptors {
                if matches!(desc.kind, crate::index2::kind::IndexKind::Btree { .. }) {
                    continue;
                }
                let was_building = desc.state == crate::index2::state::IndexState::Building;
                let backend = crate::index2::build_index2_backend_with_resolver(
                    desc,
                    &info_store,
                    Some(mgr.scalar_resolver.load_full().as_ref().clone()),
                );
                // F-50 Step 3b self-heal: for a Building descriptor, drop any
                // partial postings the crashed attempt wrote under the
                // reserved id, then re-run the full backfill. The backend is
                // freshly constructed (empty adapter / no in-memory state),
                // so `drop_all` cleans only the crashed attempt's persisted
                // postings; the backfill that follows rebuilds a complete,
                // consistent index.
                if was_building {
                    log::warn!(
                        "index2 backend '{}' (id={}) was persisted in Building state — \
                         build was interrupted by a crash; restarting the build from scratch \
                         (drop_all + full backfill)",
                        backend.descriptor().name,
                        backend.descriptor().id
                    );
                    let recovered_id = backend.descriptor().id;
                    // R0-D (#1013): a failed `drop_all` is NOT safe to paper
                    // over. Partial postings from the crashed attempt may
                    // survive alongside whatever the backfill below writes —
                    // for FTS specifically this means stats are not a clean
                    // overwrite, silently double-counting. Fail CLOSED: mark
                    // the backend `Failed` (planner-invisible, same as
                    // `Building`) and skip both the backfill and the
                    // Ready-flip, instead of proceeding as if recovery
                    // succeeded.
                    if let Err(e) = backend.drop_all().await {
                        let reason = format!(
                            "index2 drop_all during restart-from-scratch for '{}' (id={}) \
                             failed: {e} — partial postings may persist; NOT proceeding with \
                             backfill. Call TableManager::repair() once the underlying storage \
                             fault is resolved.",
                            backend.descriptor().name,
                            backend.descriptor().id
                        );
                        log::error!("{reason}");
                        let backend_name = backend.descriptor().name.clone();
                        mgr.index2_registry.insert(backend).await.map_err(|e| {
                            Self::duplicate_persisted_index2_name_error(
                                &backend_name,
                                recovered_id,
                                &e,
                            )
                        })?;
                        mgr.index2_registry.set_failed(recovered_id, reason).await;
                        failed_recovery_ids.push(recovered_id);
                        continue;
                    }
                    // All backfill errors propagate except the actionable ngram budget failure.
                    match mgr.backfill_index2_backend(backend.as_ref()).await {
                        Ok(()) => {}
                        Err(crate::index2::IndexError::TokenBudgetExceeded { .. }) => {
                            let reason = format!(
                                "index '{}' (id={recovered_id}): an indexed record exceeds the ngram token budget; shorten or delete the record, then call TableManager::repair(), or DROP INDEX. Repair alone is insufficient.",
                                backend.descriptor().name
                            );
                            log::error!("{reason}");
                            let backend_name = backend.descriptor().name.clone();
                            mgr.index2_registry.insert(backend).await.map_err(|e| {
                                Self::duplicate_persisted_index2_name_error(
                                    &backend_name,
                                    recovered_id,
                                    &e,
                                )
                            })?;
                            mgr.index2_registry.set_failed(recovered_id, reason).await;
                            failed_recovery_ids.push(recovered_id);
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    }
                    let backend_name = backend.descriptor().name.clone();
                    mgr.index2_registry.insert(backend).await.map_err(|e| {
                        Self::duplicate_persisted_index2_name_error(&backend_name, recovered_id, &e)
                    })?;
                    // Flip Building → Ready now that the backfill has
                    // completed.
                    mgr.index2_registry
                        .set_state(recovered_id, crate::index2::state::IndexState::Ready)
                        .await;
                    recovered_building_ids.push(recovered_id);
                    continue;
                }
                let desc_id = backend.descriptor().id;
                let desc_name = backend.descriptor().name.clone();
                mgr.index2_registry.insert(backend).await.map_err(|e| {
                    Self::duplicate_persisted_index2_name_error(&desc_name, desc_id, &e)
                })?;
            }
            // Re-persist so the on-disk state matches the now-Ready in-memory
            // state. Without this, a second crash before the next
            // `save_index2_metadata` would leave `Building` on disk and force
            // a redundant re-backfill on the NEXT reopen (correct but wasteful).
            if !recovered_building_ids.is_empty() {
                if let Err(e) = crate::index2::persistence::save_index2_metadata(
                    &mgr.index2_registry,
                    &mgr.info_store,
                )
                .await
                {
                    log::warn!(
                        "recovery: failed to re-persist index2 metadata after Building→Ready \
                         flip: {e}; on-disk state stays Building, forcing a redundant \
                         re-backfill on the next reopen (correct but wasteful)"
                    );
                }
            }
        }

        // P0-3b (#988): recover any index2 DROP INDEX operations interrupted
        // by a crash. Runs AFTER the F-50 Step 3b block (which loaded
        // persisted metadata and inserted backends into the registry) and
        // BEFORE the `restore_on_open` loop (so we don't waste time rebuilding
        // a backend that recovery is about to remove). Mirrors sorted's
        // `recover_in_progress_drops` (called from `SortedIndexManager::new`).
        //
        // #1069: index2 DROP recovery now writes SucceededViaCrashRecovery status
        // directly within recover_index2_drops(), BEFORE clearing the tombstone.
        // No pre-load or post-write needed here.
        // S5c-X: open-path recovery runs under the repo's own (fresh,
        // uncontended) admission permit — recover_hash_renames takes the
        // per-table write barrier internally.
        // S5c-X: open-path recovery runs under the CALLER'S admission permit
        // (already debug-checked above) — recover_hash_renames takes the
        // per-table write barrier internally. No re-admit: the domain mutex
        // is non-reentrant.
        let open_permit = permit;
        mgr.recover_index2_drops().await?;

        // #1066: recover any index2 RENAME operations interrupted by a
        // crash. Runs immediately AFTER `recover_index2_drops` — both touch
        // the SAME `index2_registry` and `save_index2_metadata` persistence
        // path (drop recovery must settle the registry's shape first), and
        // BEFORE `recover_hash_renames` below, which touches a completely
        // different subsystem (`IndexManager`, base_index hash family).
        // Also BEFORE the `restore_on_open` loop, same reasoning as the drop
        // recovery above. See `recover_index2_renames`'s doc for the full
        // recovery semantics.
        mgr.recover_index2_renames().await?;

        // #997: recover any RENAME INDEX operations on the base_index
        // regular/unique (hash) families interrupted by a crash. Runs AFTER
        // `recover_index2_drops` (same position as the index2 recovery)
        // and BEFORE the `restore_on_open` loop. Lives on `TableManager`
        // (not `IndexManager`) because a hash rename is a drop+rebuild that
        // needs the record stream + interner for a backfill. The tombstone
        // carries the resolved string names + paths so recovery can rebuild
        // from nothing — essential for the unique path, which drops the OLD
        // definition FIRST. See `recover_hash_renames`'s doc for the full
        // crash-state matrix.
        mgr.recover_hash_renames_admitted(open_permit).await?;

        // Restore in-memory state from persisted data.
        //
        // Each backend restores itself via `restore_on_open`: most
        // backends (Functional, FTS, Btree) fall through to the default
        // which is a full data-store scan `rebuild`. VectorBackend
        // overrides `restore_on_open` to try its persisted HNSW snapshot
        // FIRST (V2.2 / #401) and only fall back to a full scan when the
        // snapshot is absent/corrupt — so a warm restart is O(load), not
        // O(N-scan).
        //
        // F-50 Step 3b: a backend that was JUST recovered from `Building`
        // (in the loop above) is skipped here — its backfill already
        // populated it (postings AND in-memory stats), so a second
        // `restore_on_open` rebuild would double-count FTS `BumpFtsStats`
        // and needlessly re-scan. The `recovered_building_ids` set carries
        // the recovered ids forward.
        {
            let backends = mgr.index2_registry.all_backends().await;
            for b in &backends {
                let id = b.descriptor().id;
                if recovered_building_ids.contains(&id) || failed_recovery_ids.contains(&id) {
                    continue;
                }
                let info = Arc::clone(&mgr.info_store);
                let data = Arc::clone(mgr.table.data_store());
                // R0-D (#1013): a failed `restore_on_open` must NOT leave the
                // backend registered as if recovery succeeded. Before this
                // fix, the code logged a warning and moved on — the backend
                // stayed at whatever in-memory state its freshly-constructed
                // adapter started with (empty, for most backends), silently
                // serving incomplete/empty results with no error signal.
                // Fail CLOSED: mark the backend `Failed` (planner-invisible,
                // counted by `degraded_index_count()`) so a client sees "no
                // usable index" instead of a confident-looking wrong answer.
                if let Err(e) = b.restore_on_open(info, data).await {
                    let reason = format!(
                        "index2 restore_on_open failed for index '{}' (id={id}): {e} — the \
                         backend may be empty/incomplete; marking Failed instead of serving \
                         possibly-incomplete results. Call TableManager::repair() once the \
                         underlying storage fault is resolved.",
                        b.descriptor().name
                    );
                    log::error!("{reason}");
                    mgr.index2_registry.set_failed(id, reason).await;
                }
            }
        }

        // Crash recovery is owned by the repo-level file WAL replay
        // (`RepoInstance::recover_v2_inflight`), which runs on repo open.
        // The legacy per-table KV-WAL scan that used to live here was
        // removed in F5d: after the non-tx write cutover (F4b/F5a) the
        // per-table WAL is no longer written, so this scan was a no-op.

        // S9b (#81): legacy-index format-v2 rebuild-on-open. If the stored
        // posting format version is older than current (or absent = pre-S9
        // data), the on-disk hash scheme is V1 and would yield silent lookup
        // misses against the V2 hasher. Rebuild every legacy posting
        // (hash/unique/sorted) from the data store, then stamp the version so
        // subsequent opens are a single cheap version read. The full O(N)
        // scan is skipped when the table has no legacy indexes — only the
        // marker is written.
        if crate::index2::persistence::legacy_indexes_need_rebuild(&mgr.info_store).await? {
            let has_legacy = mgr.index_manager_ref().iter_indexes().next().is_some()
                || mgr
                    .index_manager_ref()
                    .iter_unique_indexes()
                    .next()
                    .is_some()
                || !mgr.sorted_indexes().iter_indexes().is_empty();
            if has_legacy {
                // S5c-X: the open path already holds `open_permit` — use the
                // admitted twin (public `repair()` would re-admit and
                // self-deadlock on the non-reentrant domain mutex).
                mgr.repair_admitted(open_permit).await?;
            }
            crate::index2::persistence::save_legacy_index_version(&mgr.info_store).await?;
        }

        // #1068: catch up the DDL op-log FIFO retention sweep at open time.
        // The throttled trigger in `ddl_op_log::write_op_status` lives behind
        // an in-memory `AtomicU64` that resets on every restart, so a table
        // that was mid-way through its throttle window at crash time would
        // otherwise not get swept again until `DDL_OP_LOG_EVICTION_CHECK_INTERVAL`
        // more terminal writes accumulate. Mirrors the other periodic
        // maintenance sweeps run at open (e.g. the legacy-index rebuild
        // above) — not itself a crash-recovery path, just a good moment to
        // catch up. Failure is logged, not fatal: an unswept log is a
        // capacity/observability concern, not a correctness one, and must
        // not block the table from opening.
        if let Err(e) =
            crate::table::ddl_op_log::maybe_evict_terminal_records(&mgr.info_store).await
        {
            log::error!(
                "#1068: DDL op-log eviction catch-up sweep at table open failed for table \
                 '{}': {e}",
                mgr.name
            );
        }

        Ok(mgr)
    }
}
