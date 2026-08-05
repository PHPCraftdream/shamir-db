use std::sync::Arc;

use super::buffer_config;
use super::interner_manager::InternerManager;
use super::persistable::PersistRegistry;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::SortedIndexManager;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;

/// Compute the deterministic token for a table name.
/// Same hash as `TableManager::table_token` (instance method).
pub fn table_token_for(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

pub struct TableManager {
    pub(super) name: String,
    pub(super) table: Arc<Table>,
    /// Direct handle to the info_store the sub-managers were
    /// built on. Kept so DDL (buffer-config get/set, future
    /// per-table settings) can hit the same store without going
    /// through any sub-manager's surface.
    pub(super) info_store: Arc<dyn Store>,
    pub(super) interner: InternerManager,
    pub(super) counter: Arc<RecordCounter>,
    /// Registry of metadata blobs that are flushed together at the end
    /// of each write operation via `flush_metadata()`.
    pub(super) persist_registry: PersistRegistry,
    pub(super) index_manager: IndexManager,
    pub(super) sorted_indexes: SortedIndexManager,
    /// Monotonic counter of mutating operations since open. The
    /// auto-verify background watchdog samples this; every
    /// `AUTO_VERIFY_EVERY_N_WRITES` operations it spawns a verify
    /// pass and logs anything unhealthy. See `bump_write_counter`.
    pub(super) write_counter: Arc<std::sync::atomic::AtomicU64>,
    /// `true` when a background verify is in flight — prevents
    /// multiple concurrent verifies piling up.
    pub(super) verify_running: Arc<std::sync::atomic::AtomicBool>,
    /// Serialises validate + write + index-update for tables that have
    /// unique indexes. Tables without unique indexes hit the fast path
    /// (no lock). `tokio::sync::Mutex` because the guard lives across
    /// `.await` points.
    pub(super) unique_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// F-69 (#896, P0): the SINGLE packed atomic backing the whole
    /// write-barrier predicate — see
    /// [`write_barrier_flags`](crate::index::write_barrier_flags) for the
    /// full rationale (why one `Arc<AtomicU8>` replaces what used to be six
    /// independent atomics: five `AtomicBool`s here plus
    /// `IndexManager::has_unique_indexes()`'s own flag).
    ///
    /// This is the SAME `Arc<AtomicU8>` [`IndexManager`] uses for its
    /// `UNIQUE_INDEX_EXISTS` bit — obtained via
    /// [`IndexManager::write_barrier_flags`] at construction time and
    /// stored here so `TableManager` can fold its own five DDL-intent bits
    /// into the identical word:
    ///
    /// - `INDEX2_CREATE` — `true` (under `unique_write_lock`) for the
    ///   duration of an index2 (`fts`/`functional`/`vector`)
    ///   `create_index_v2` backfill→register sequence. While set, EVERY
    ///   writer path also acquires `unique_write_lock`, so no row can be
    ///   written in the backfill→register window where it would be seen by
    ///   NEITHER the backfill snapshot (cursor already past it) NOR the live
    ///   `index2_on_insert` hook (backend not yet registered) — the
    ///   lost-write race (#534, finding 1) that `unique_write_lock` alone
    ///   does not close because writers only took that lock when a base_index
    ///   unique index existed, and an index2-only table has none.
    /// - `SCHEMA_ACTIVATION` (F-37, #845) — raised for the duration of a
    ///   schema-activation DDL sequence (`set_table_schema` /
    ///   `add_schema_rule`) whose `keyset_safe` proof reads
    ///   `table.count() == 0` and then persists + activates a new schema
    ///   rule. While set, EVERY writer path also acquires
    ///   `unique_write_lock`, so no row can land between the count proof and
    ///   the schema's activation — the F-37 race that `lock_schema_rmw` does
    ///   NOT close (that lock only serializes schema DDL against OTHER
    ///   schema DDL in `admin_user_locks`, never against the write path in
    ///   `table_manager_crud.rs`). Independently settable/clearable from
    ///   `INDEX2_CREATE`. Set/cleared by the shamir-db DDL handler via
    ///   [`set_schema_activation_barrier`](Self::set_schema_activation_barrier).
    /// - `REGULAR_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   regular (non-unique) hash-index `create_index`
    ///   snapshot→backfill→register sequence, closing the lost-write race
    ///   where a row written between the backfill snapshot and registration
    ///   is seen by NEITHER the backfill NOR the live `index_manager` write
    ///   hook (`needs_write_barrier()` previously never considered regular
    ///   indexes at all). Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    /// - `UNIQUE_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   unique-index `create_unique_index` snapshot→backfill→register
    ///   sequence. The `UNIQUE_INDEX_EXISTS` bit already forces the barrier
    ///   once a unique index is registered, but for the FIRST unique index on
    ///   a table that bit is unset until the index IS registered — so every
    ///   concurrent fast-path writer would otherwise bypass the lock
    ///   entirely. Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    /// - `SORTED_INDEX_CREATE` (F-57, #883) — raised for the duration of a
    ///   sorted-index `create_sorted_index_with_include` register→backfill
    ///   sequence. Sorted indexes register BEFORE backfill (load-bearing: the
    ///   backfill calls `on_record_created` through the manager, which needs
    ///   the registered definition). Without this bit, a concurrent writer
    ///   could interleave with the backfill and the row could be missed or
    ///   double-posted. Set/cleared by [`WriteBarrierGuard`] (F-70, #897).
    ///
    /// Every bit-set/clear is `SeqCst` (F-56's total-order argument, now
    /// covering the FULL six-condition predicate — see
    /// [`writer_drain_barrier`]'s module doc, updated by this task).
    pub(super) write_barrier_flags: crate::index::write_barrier_flags::WriteBarrierFlags,
    /// F-95 (#957, P0) — per-table DDL admission mutex. Serializes
    /// concurrent DDL operations that would share the same write-barrier
    /// bit (e.g. two `CREATE INDEX` both raising `REGULAR_INDEX_CREATE`).
    /// Acquired FIRST inside
    /// [`begin_write_barrier`](Self::begin_write_barrier) (before the
    /// raise→drain→lock sequence) and held for the caller's ENTIRE
    /// critical section (the guard is embedded in the returned
    /// [`WriteBarrierGuard`]).
    ///
    /// Without this, two concurrent DDLs sharing a bit race: DDL-A drops
    /// its `WriteBarrierGuard` (unconditionally clearing the bit via
    /// `fetch_and`) while DDL-B has already drained and is parked on
    /// `unique_write_lock`. When B acquires the lock the bit is 0 and B
    /// never re-raises it, so a brand-new writer fast-paths past B's
    /// still-in-progress backfill — the exact hazard the barrier exists
    /// to prevent.
    ///
    /// The admission mutex ensures DDL-B does its OWN raise→drain→lock
    /// from scratch, AFTER DDL-A's entire critical section (including
    /// guard drop) has completed. Regular writers
    /// ([`needs_write_barrier`](Self::needs_write_barrier)) do NOT take
    /// this mutex — only `begin_write_barrier` callers do — so the F-70
    /// (#897) lock-order invariant (writers never block behind a
    /// DDL-held lock in a way that could deadlock) is preserved.
    ///
    /// Lock-order hierarchy (extending F-70's canonical order):
    /// `ddl_admission` → (raise bit) → `drain_writers` →
    /// `unique_write_lock`. No writer or committer ever acquires
    /// `ddl_admission`, so no cycle involving it can form.
    pub(super) ddl_admission: Arc<tokio::sync::Mutex<()>>,
    /// F-48 (#859, P0) — reusable writer-drain barrier. Fast-path writers
    /// (those that read `needs_write_barrier() == false`) bump this counter
    /// before their flag check and drop it after their full
    /// validate→write→index sequence. A drainer (schema DDL today; index2
    /// create in F-50) calls [`drain_writers`](Self::drain_writers) after
    /// raising its intent flag + acquiring `unique_write_lock` to wait for
    /// any in-flight fast-path writer that read `false` before the flag went
    /// up — the check-then-act gap `unique_write_lock` + the flag alone
    /// cannot close. Shared across clones via `Arc` (same rationale as the
    /// sibling barrier flags). See [`writer_drain_barrier`] for the full
    /// memory-model + reusability rationale.
    pub(super) writer_drain: super::writer_drain_barrier::WriterDrainBarrier,
    pub(super) index2_registry: Arc<crate::index2::IndexRegistry>,
    pub(super) mvcc_store: Option<Arc<shamir_tx::MvccStore>>,
    /// Per-table validator bindings (S2). Lock-free reads via
    /// `ArcSwap`; the S3 write path reads this on every write.
    /// DDL (`add_validator_binding` / `remove_validator_binding`)
    /// mutates + persists to the info-twin.
    pub(super) validator_bindings: Arc<arc_swap::ArcSwap<Vec<crate::validator::ValidatorBinding>>>,
    /// Mirror of `validator_bindings.load_full().len()` for the hot-path
    /// fast skip. Allows `run_validators_qv`/`run_validators_view` to
    /// early-return on the common "no validators bound" case without paying
    /// for an `ArcSwap::load_full()` Arc-clone. Updated atomically in
    /// `add_validator_binding`/`remove_validator_binding` after the ArcSwap
    /// store (Release ordering — pairs with Acquire in the fast-skip load).
    ///
    /// **Shared across clones via `Arc`** — `TableManager` is cloned
    /// value-by-`get_table().cloned()` on every consumer (DDL bind path
    /// AND the write path each get their own copy). A per-instance
    /// `AtomicUsize` would desync: `add_validator_binding` on a bind-path
    /// clone would bump only that clone's counter, while the next
    /// write-path clone (snapshotted from the OnceCell primary, whose
    /// counter was never bumped) still sees 0 → fast-skip → validators
    /// silently never fire. Wrapping in `Arc` makes all clones observe the
    /// same single counter, restoring the intended invariant that any
    /// clone's `add`/`remove` is visible to every other clone's fast-skip
    /// load. This mirrors how `validator_bindings` itself stays shared
    /// through `Arc<ArcSwap<..>>`.
    pub(super) bindings_len: Arc<std::sync::atomic::AtomicUsize>,
    /// Handle to the global validator registry (S3). `None` for system
    /// tables / tests that don't need validation. The S3 write path
    /// reads this to resolve `ValidatorBinding.validator_id` to a
    /// compiled `ShamirFunction`.
    pub(super) validator_registry: Option<Arc<crate::validator::ValidatorRegistry>>,
    /// SSI gate handle — wires the non-tx write path to the per-repo
    /// [`RepoTxGate`](shamir_tx::RepoTxGate) so that Serializable
    /// transactions see non-tx writes in their Phase 2-bis predicate-conflict
    /// check. `None` for system tables / tests that have no gate wired.
    /// Attached by `RepoInstance::create_table_context` via
    /// [`with_changefeed`](Self::with_changefeed).
    pub(super) changefeed: Option<NonTxChangefeed>,
    /// Per-DB scalar resolver (user + builtin layers). Lock-free reads
    /// via `ArcSwap`; defaults to builtins-only until
    /// [`set_scalar_resolver`](Self::set_scalar_resolver) is called.
    /// Used by `create_index_v2` for the `.trusted_pure()` index-safety gate.
    pub(super) scalar_resolver:
        Arc<arc_swap::ArcSwap<shamir_funclib::scalar_resolver::ScalarResolver>>,
    /// Test-only deterministic pause point for `create_index_v2`, installed
    /// between the index2 backfill and the `index2_registry.insert`. Lets a
    /// #534-finding-1 regression test drive a concurrent writer INTO the exact
    /// lost-write window (backfill done, backend not yet registered) so the
    /// test fails deterministically against the pre-fix (barrier-less) code and
    /// passes after. `None` in every non-test build and by default in tests —
    /// zero cost on the real create path.
    #[cfg(test)]
    pub(super) create_index2_backfill_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// F-72 (#899, P0) test-only deterministic pause point: parks
    /// `create_sorted_index_with_include`'s backfill loop mid-stream (after
    /// registering the definition at `Building`, before it flips to
    /// `Ready`). Lets a regression test drive a concurrent READ into the
    /// exact planner-invisibility window this task closes — see
    /// `f72_planner_invisibility_tests.rs`. Reuses the same
    /// `BackfillPauseHook` primitive as `create_index2_backfill_hook`.
    /// `None` in every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) create_sorted_index_backfill_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// F-76 (#903) test-only deterministic pause point: parks `drop_index2`
    /// between the registry retirement and the posting sweep (the exact
    /// visibility window this task closes). Lets a regression test drive a
    /// concurrent READ into the window and assert it falls back to a full
    /// scan instead of observing a registered-but-emptied backend. Reuses
    /// the same `BackfillPauseHook` primitive. `None` in every non-test
    /// build and by default in tests — see `f76_drop_visibility_tests.rs`.
    #[cfg(test)]
    pub(super) drop_index2_pause_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
    /// P0-3b (#988) test-only deterministic pause point: parks `drop_index2`
    /// AFTER the posting sweep but BEFORE the reduced metadata is persisted —
    /// the exact crash window sub-bug 3c exercises. A regression test
    /// installs this hook, parks `drop_index2` mid-drop, drops the manager
    /// (simulating a crash), then constructs a fresh `TableManager::create`
    /// against the SAME stores and asserts the recovery path finishes the
    /// drop. Mirrors sorted's `drop_index_post_sweep_hook` (#972). `None` in
    /// every non-test build and by default in tests.
    #[cfg(test)]
    pub(super) drop_index2_post_sweep_hook:
        Arc<arc_swap::ArcSwapOption<super::index2_backfill_hook::BackfillPauseHook>>,
}

/// Bundle wiring the non-tx write path to the SSI commit-write log.
///
/// Holds the per-repo [`RepoTxGate`](shamir_tx::RepoTxGate) (the version
/// source — shared with the tx commit pipeline so versions stay monotonic
/// across both paths). The `gate` is used by `record_nontx_ssi_footprint`
/// to append `CommitWriteRecord`s so that Serializable transactions see
/// non-tx writes in their Phase 2-bis predicate-conflict window.
/// Cloned cheaply (`Arc`).
#[derive(Clone)]
pub(super) struct NonTxChangefeed {
    pub(super) gate: Arc<shamir_tx::RepoTxGate>,
}

/// How often the background watchdog runs a `verify` pass.
/// Coarse — once per ~thousand mutating ops, regardless of batch
/// size. Tuned to "noticeable problem within seconds" without
/// noticeable overhead.
pub(super) const AUTO_VERIFY_EVERY_N_WRITES: u64 = 1024;

impl Clone for TableManager {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            table: Arc::clone(&self.table),
            info_store: Arc::clone(&self.info_store),
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            persist_registry: self.persist_registry.clone(),
            index_manager: self.index_manager.clone(),
            sorted_indexes: self.sorted_indexes.clone(),
            write_counter: Arc::clone(&self.write_counter),
            verify_running: Arc::clone(&self.verify_running),
            unique_write_lock: Arc::clone(&self.unique_write_lock),
            // F-69 (#896) — shared across clones (Arc<AtomicU8> inside) so a
            // create/DDL in flight on any clone forces writers on every clone
            // onto the barrier, same rationale the five separate flags this
            // replaces each carried individually.
            write_barrier_flags: self.write_barrier_flags.clone(),
            // F-95 (#957) — shared across clones (Arc<tokio::sync::Mutex>
            // inside) so a DDL admission in flight on any clone blocks every
            // clone's begin_write_barrier, same rationale as the other
            // shared-Arc fields.
            ddl_admission: Arc::clone(&self.ddl_admission),
            // F-48 — shared across clones (Arc<AtomicUsize> inside).
            writer_drain: self.writer_drain.clone(),
            index2_registry: Arc::clone(&self.index2_registry),
            mvcc_store: self.mvcc_store.clone(),
            validator_bindings: Arc::clone(&self.validator_bindings),
            // Clone shares the SAME Arc<AtomicUsize> so any clone's
            // add/remove_validator_binding (which stores Release) is visible
            // to every clone's fast-skip load (Acquire). A snapshot-copy
            // here would desync the primary from bind-path clones (see the
            // field doc on `bindings_len` for the regression this caused).
            bindings_len: Arc::clone(&self.bindings_len),
            validator_registry: self.validator_registry.clone(),
            changefeed: self.changefeed.clone(),
            scalar_resolver: Arc::clone(&self.scalar_resolver),
            #[cfg(test)]
            create_index2_backfill_hook: Arc::clone(&self.create_index2_backfill_hook),
            #[cfg(test)]
            create_sorted_index_backfill_hook: Arc::clone(&self.create_sorted_index_backfill_hook),
            #[cfg(test)]
            drop_index2_pause_hook: Arc::clone(&self.drop_index2_pause_hook),
            #[cfg(test)]
            drop_index2_post_sweep_hook: Arc::clone(&self.drop_index2_post_sweep_hook),
        }
    }
}

impl TableManager {
    /// Create a new TableManager with all internal components.
    ///
    /// This is the preferred way to create a TableManager - it handles
    /// internal Table creation and all component initialization.
    pub async fn create(
        name: String,
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
    ) -> DbResult<Self> {
        let interner = InternerManager::new(Arc::clone(&info_store));
        let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));

        // Build the persist registry — cloning interner shares all its
        // internal Arcs (same underlying data), wrapping in Arc<dyn Persistable>
        // gives the uniform flush surface.
        let mut persist_registry = PersistRegistry::new();
        persist_registry
            .register(Arc::new(interner.clone()) as Arc<dyn super::persistable::Persistable>);
        persist_registry.register(Arc::clone(&counter) as Arc<dyn super::persistable::Persistable>);

        let index_manager =
            IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store)).await?;
        // F-69 (#896): clone the SAME packed word `index_manager` uses for its
        // `UNIQUE_INDEX_EXISTS` bit — `TableManager` folds its own five
        // DDL-intent bits into this identical `Arc<AtomicU8>`, below.
        let write_barrier_flags = index_manager.write_barrier_flags();
        let sorted_indexes = SortedIndexManager::new(Arc::clone(&info_store)).await?;
        let table = Table::new(data_store);

        // Pre-load validator bindings from the info-twin (S2).
        let (validator_bindings, initial_bindings_len) =
            match crate::validator::persistence::load_validators_metadata(&info_store).await? {
                Some(pv) => {
                    let len = pv.bindings.len();
                    (Arc::new(arc_swap::ArcSwap::from_pointee(pv.bindings)), len)
                }
                None => (Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())), 0),
            };

        let mgr = Self {
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
            writer_drain: super::writer_drain_barrier::WriterDrainBarrier::new(),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
            validator_bindings,
            bindings_len: Arc::new(std::sync::atomic::AtomicUsize::new(initial_bindings_len)),
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
                    if let Err(e) = backend.drop_all().await {
                        // `drop_all` failure is not fatal: the backfill below
                        // will re-write postings idempotently for most
                        // backends (functional/vector). Log and continue —
                        // matching `restore_on_open`'s own error policy.
                        log::warn!(
                            "index2 drop_all during restart-from-scratch for '{}' (id={}) \
                             failed: {} — continuing with backfill (partial postings may persist)",
                            backend.descriptor().name,
                            backend.descriptor().id,
                            e
                        );
                    }
                    // Re-run the backfill (the same `backfill_index2_backend`
                    // `create_index_v2` uses). Errors propagate: a backfill
                    // failure on reopen is a genuine data-integrity problem,
                    // not a transient issue.
                    mgr.backfill_index2_backend(backend.as_ref()).await?;
                }
                let recovered_id = backend.descriptor().id;
                let _ = mgr.index2_registry.insert(backend).await;
                // Flip Building → Ready now that the backfill has completed
                // (for Ready descriptors this is a no-op — their tuple slot
                // already carries Ready from `insert`).
                if was_building {
                    mgr.index2_registry
                        .set_state(recovered_id, crate::index2::state::IndexState::Ready)
                        .await;
                    recovered_building_ids.push(recovered_id);
                }
            }
            // Re-persist so the on-disk state matches the now-Ready in-memory
            // state. Without this, a second crash before the next
            // `save_index2_metadata` would leave `Building` on disk and force
            // a redundant re-backfill on the NEXT reopen (correct but wasteful).
            if !recovered_building_ids.is_empty() {
                let _ = crate::index2::persistence::save_index2_metadata(
                    &mgr.index2_registry,
                    &mgr.info_store,
                )
                .await;
            }
        }

        // P0-3b (#988): recover any index2 DROP INDEX operations interrupted
        // by a crash. Runs AFTER the F-50 Step 3b block (which loaded
        // persisted metadata and inserted backends into the registry) and
        // BEFORE the `restore_on_open` loop (so we don't waste time rebuilding
        // a backend that recovery is about to remove). Mirrors sorted's
        // `recover_in_progress_drops` (called from `SortedIndexManager::new`).
        mgr.recover_index2_drops().await?;

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
        mgr.recover_hash_renames().await?;

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
                if recovered_building_ids.contains(&b.descriptor().id) {
                    continue;
                }
                let info = Arc::clone(&mgr.info_store);
                let data = Arc::clone(mgr.table.data_store());
                if let Err(e) = b.restore_on_open(info, data).await {
                    log::warn!(
                        "index2 restore_on_open failed for index {}: {}",
                        b.descriptor().name,
                        e
                    );
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
                mgr.repair().await?;
            }
            crate::index2::persistence::save_legacy_index_version(&mgr.info_store).await?;
        }

        Ok(mgr)
    }

    /// Create a TableManager from existing components.
    ///
    /// This is primarily for testing or advanced use cases.
    #[cfg(test)]
    pub fn new(
        name: String,
        table: Table,
        interner: InternerManager,
        counter: Arc<RecordCounter>,
        index_manager: IndexManager,
    ) -> Self {
        // Tests that construct TableManager directly don't exercise
        // sorted indexes — give them an empty manager that shares
        // info_store... but we don't have it here. The simplest
        // thing: construct an "orphan" sorted manager backed by an
        // in-memory store. Its persisted defs blob then lives in a
        // throwaway store, which is fine because these tests never
        // call sorted-index methods.
        let info_store: Arc<dyn Store> =
            Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
        // Construct synchronously: SortedIndexManager::new() is async
        // but the empty-state path doesn't await any real work.
        let sorted_indexes =
            futures::executor::block_on(SortedIndexManager::new(info_store.clone()))
                .expect("sorted index manager init for test");
        let mut persist_registry = PersistRegistry::new();
        persist_registry
            .register(Arc::new(interner.clone()) as Arc<dyn super::persistable::Persistable>);
        persist_registry.register(Arc::clone(&counter) as Arc<dyn super::persistable::Persistable>);
        // F-69 (#896): clone the SAME packed word `index_manager` uses for its
        // `UNIQUE_INDEX_EXISTS` bit before `index_manager` moves into `Self`.
        let write_barrier_flags = index_manager.write_barrier_flags();
        Self {
            name,
            table: Arc::new(table),
            info_store,
            interner,
            counter,
            persist_registry,
            index_manager,
            sorted_indexes,
            write_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            verify_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unique_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_barrier_flags,
            ddl_admission: Arc::new(tokio::sync::Mutex::new(())),
            writer_drain: super::writer_drain_barrier::WriterDrainBarrier::new(),
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
            validator_bindings: Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())),
            bindings_len: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        }
    }

    /// Increment the watchdog counter for `n` writes. Every
    /// `AUTO_VERIFY_EVERY_N_WRITES`-th increment spawns a
    /// non-blocking background `verify()` and logs at WARN if it
    /// reports inconsistency. Best-effort signal — does NOT block
    /// the caller, does NOT auto-repair (user calls `repair()`
    /// when ready).
    pub fn bump_write_counter(&self, n: u64) {
        use std::sync::atomic::Ordering;
        if n == 0 {
            return;
        }
        let prev = self.write_counter.fetch_add(n, Ordering::Relaxed);
        let next = prev.saturating_add(n);
        let crossed = prev / AUTO_VERIFY_EVERY_N_WRITES != next / AUTO_VERIFY_EVERY_N_WRITES;
        if !crossed {
            return;
        }
        // Single-flight: skip if another verify is in flight.
        if self
            .verify_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let self_clone = self.clone();
        tokio::spawn(async move {
            let result = self_clone.verify().await;
            match result {
                Ok(report) => {
                    if !report.is_healthy() {
                        log::warn!(
                            "Background verify flagged inconsistency in '{}': {:?}",
                            self_clone.name(),
                            report,
                        );
                    }
                }
                Err(e) => log::warn!("Background verify on '{}' failed: {}", self_clone.name(), e,),
            }
            self_clone.verify_running.store(false, Ordering::Release);
        });
    }

    /// Whether a background verify is currently in flight. Test-
    /// support accessor; users normally don't care.
    pub fn is_background_verify_running(&self) -> bool {
        use std::sync::atomic::Ordering;
        self.verify_running.load(Ordering::Acquire)
    }

    /// Public read-only access to the inner `Table` — used by
    /// `read_exec` for vectored `get_many` and by tests. Production
    /// callers must not write to the table directly; go through
    /// `TableManager::insert / set / delete` so index hooks fire.
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// Stable u64 identifier for this table, used as key in
    /// `TxContext.write_set` and `counter_deltas`.
    ///
    /// Stage 4 implementation: deterministic hash of `self.name`.
    /// Stage 5 will replace with real repo-level interner ID.
    pub fn table_token(&self) -> u64 {
        table_token_for(&self.name)
    }

    /// Direct access to the underlying data_store. Used by V2 WAL
    /// recovery to apply Put/Delete ops bypassing the indexing /
    /// counter hooks (those replay separately).
    pub fn data_store(&self) -> &Arc<dyn Store> {
        self.table.data_store()
    }

    /// Borrow the info_store this table writes its sidecar
    /// metadata into (counter, interner dictionary, sorted-index
    /// blob, WAL, buffer config, ...). DDL uses this directly.
    pub fn info_store(&self) -> &Arc<dyn Store> {
        &self.info_store
    }

    pub fn interner(&self) -> &InternerManager {
        &self.interner
    }

    /// Public accessor for the record counter — used by the read
    /// fast-path for `COUNT(*)` without filter (Opt #2).
    pub fn counter(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    #[cfg(test)]
    pub fn index_manager(&self) -> &IndexManager {
        &self.index_manager
    }

    /// Borrow the table's `IndexManager`. Public so the `db_instance`
    /// admin path (`create_index_async`) can register / drop indices via
    /// `TableManager` from outside this module — previously `pub(crate)`
    /// when this code was a single crate, but `db_instance` and
    /// `table_manager` now live in adjacent crate modules and the
    /// boundary needs `pub`.
    pub fn index_manager_ref(&self) -> &IndexManager {
        &self.index_manager
    }

    pub fn index2_registry(&self) -> &Arc<crate::index2::IndexRegistry> {
        &self.index2_registry
    }

    /// Test-only: install (or clear with `None`) the deterministic
    /// `create_index_v2` backfill→register pause hook (#534 finding 1).
    #[cfg(test)]
    pub(crate) fn set_create_index2_backfill_hook(
        &self,
        hook: Option<Arc<super::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.create_index2_backfill_hook.store(hook);
    }

    /// F-72 (#899, P0) test-only: install (or clear with `None`) the
    /// deterministic `create_sorted_index_with_include` backfill pause hook.
    /// See `create_sorted_index_backfill_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_create_sorted_index_backfill_hook(
        &self,
        hook: Option<Arc<super::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.create_sorted_index_backfill_hook.store(hook);
    }

    /// F-76 (#903) test-only: install (or clear with `None`) the deterministic
    /// `drop_index2` pause hook (fires between the registry retirement and the
    /// posting sweep). See `drop_index2_pause_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_drop_index2_pause_hook(
        &self,
        hook: Option<Arc<super::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.drop_index2_pause_hook.store(hook);
    }

    /// P0-3b (#988) test-only: install (or clear with `None`) the deterministic
    /// `drop_index2` post-sweep pause hook (fires after the posting sweep,
    /// before the reduced metadata persist). See
    /// `drop_index2_post_sweep_hook`'s field doc.
    #[cfg(test)]
    pub(crate) fn set_drop_index2_post_sweep_hook(
        &self,
        hook: Option<Arc<super::index2_backfill_hook::BackfillPauseHook>>,
    ) {
        self.drop_index2_post_sweep_hook.store(hook);
    }

    /// Clone the handle to this table's unique-write serialisation lock.
    ///
    /// **Physical authority for uniqueness (HIGH-A) — defense-in-depth contract
    /// (②.3b).** This `tokio::sync::Mutex` is the serialiser that closes the
    /// non-tx ↔ tx-commit unique race; the logical fail-fast probe in
    /// `schema_validator.rs` (`Phase C3 — unique constraint`) runs ABOVE this
    /// layer as early diagnosis + a clean field-scoped error, but is NOT the
    /// atomicity authority (it has a pre-commit TOCTOU window).
    ///
    /// HIGH-A — closing the non-tx ↔ tx-commit unique race. Non-tx
    /// `insert` / `set` / `delete` take this `tokio::sync::Mutex` around their
    /// validate-then-write-then-index window (see those methods). The tx
    /// commit pipeline (`commit_tx_inner` Phase 2.6 → 5c) acquires the SAME
    /// lock for every table that has unique guards, so a tx's unique
    /// re-check and its posting write are atomic against any concurrent non-tx
    /// unique writer to that table. Returns the `Arc` (cloned) so the caller
    /// holds the exact same mutex instance the non-tx path locks.
    pub fn unique_write_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.unique_write_lock)
    }

    /// F-70 (#897, P0) — THE canonical acquisition path for a DDL-side write
    /// barrier: raise `bit`, [`drain_writers`](Self::drain_writers), THEN
    /// acquire [`unique_write_lock`](Self::unique_write_lock) — in that order,
    /// and no other. See [`writer_drain_barrier`](super::writer_drain_barrier)'s
    /// module doc ("F-70 — THE canonical lock-order hierarchy") for the full
    /// derivation of why this order (not lock-then-drain, which F-57, #883,
    /// wired into every DDL create path and which this task found genuinely
    /// deadlocks against the tx-commit path's own drain-guard-then-lock
    /// shape).
    ///
    /// Every DDL call site that used to hand-roll
    /// `unique_write_lock().lock().await` followed by
    /// `IndexCreateBarrierGuard::set(..)` + `drain_writers().await` now calls
    /// this instead, so the order cannot silently drift back at a new call
    /// site (exactly how F-57 introduced the inversion in the first place —
    /// the hierarchy was never written down anywhere shared, only implied
    /// per-call-site).
    ///
    /// Returns the RAII [`WriteBarrierGuard`] (clears `bit` on drop) and the
    /// owned `unique_write_lock` guard. Both must be held for the ENTIRE
    /// snapshot/backfill/register/persist body that follows — drop order
    /// (guard before lock, achieved by dropping in the order returned/
    /// declared) mirrors every existing DDL site's discipline.
    ///
    /// `pub` (not `pub(crate)`): the schema-activation DDL handler
    /// (`shamir-db::execute::admin_schema`) is a DIFFERENT crate and must use
    /// this SAME entry point for `SCHEMA_ACTIVATION` — see
    /// `write_barrier_flags`'s bit constants, all of which are `pub`.
    pub async fn begin_write_barrier(
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
        let guard = WriteBarrierGuard::set(self.write_barrier_flags.clone(), bit, admission);
        // Step 2: drain every writer that read the flag as `false` BEFORE
        // step 1 and is still mid-flight. Cheap (one SeqCst load) when none
        // are active.
        self.drain_writers().await;
        // Step 3: ONLY NOW acquire the lock — see the module doc for why
        // this order (not the reverse) is what closes the F-70 cycle.
        let lock_guard = self.unique_write_lock.clone().lock_owned().await;
        (guard, lock_guard)
    }

    /// F-37 (#845) — drive the schema-activation write barrier from outside
    /// the engine crate (the shamir-db schema DDL handler in
    /// `admin_schema.rs`). When `on == true`, every writer consulting
    /// [`needs_write_barrier`](Self::needs_write_barrier) returns `true` and
    /// serializes on [`unique_write_lock`](Self::unique_write_lock) — closing
    /// the `keyset_safe` count-proof race (a concurrent INSERT/UPDATE can no
    /// longer land between the `count() == 0` read and the schema's persist +
    /// activate).
    ///
    /// `SeqCst`-ordered (F-56): the store must participate in the SAME single
    /// SeqCst total order as the writer's `SeqCst` flag load in
    /// [`needs_write_barrier`](Self::needs_write_barrier) and the writer's
    /// `SeqCst` `active`-counter `fetch_add` — the cross-atomic dependency the
    /// drain proof relies on cannot be carried by `Release`/`Acquire` alone
    /// (see [`writer_drain_barrier`]).
    ///
    /// **This is a raw, test-facing flag setter, NOT the production entry
    /// point.** It does NOT need to be (and in production is NOT) called
    /// under `unique_write_lock`: F-70 (#897) established the canonical
    /// drain-then-lock order — raise the intent bit FIRST, then drain, then
    /// acquire the lock (see [`begin_write_barrier`](Self::begin_write_barrier)
    /// for the sequence and [`writer_drain_barrier`](crate::table::writer_drain_barrier)'s
    /// "F-70 — THE canonical lock-order hierarchy" module doc for why the
    /// prior lock-then-set discipline is a deadlock). Production DDL callers
    /// MUST go through [`begin_write_barrier`](Self::begin_write_barrier),
    /// which performs the raise→drain→lock sequence in this exact order and
    /// returns an RAII [`WriteBarrierGuard`] that clears the bit on drop
    /// (every exit path). This raw setter remains only for tests that
    /// exercise the flag transition directly without the full barrier
    /// sequence.
    pub fn set_schema_activation_barrier(&self, on: bool) {
        self.write_barrier_flags
            .set_to(crate::index::write_barrier_flags::SCHEMA_ACTIVATION, on);
    }

    /// F-48 (#859, P0) — drain every in-flight fast-path writer before the
    /// caller proceeds past a snapshot/proof point (the `keyset_safe`
    /// count-proof for schema activation; the backfill snapshot for index2
    /// create in F-50).
    ///
    /// The caller MUST have already raised its intent bit
    /// (`SCHEMA_ACTIVATION` / `INDEX2_CREATE` / ... — see
    /// `write_barrier_flags` field doc) so NEW writers take the slow
    /// (locked) path. Then this catches any writer that read `false` before
    /// the flag went up and is still in its validate→write→index sequence.
    /// Returns immediately (one `SeqCst` load) when no fast-path writer is
    /// active.
    ///
    /// **F-70 (#897) order — call this BEFORE acquiring `unique_write_lock`,
    /// NOT after.** This is the canonical drain-then-lock order; the prior
    /// lock-then-drain order (F-57) deadlocks against the tx-commit path's
    /// drain-guard-then-lock shape — see
    /// [`writer_drain_barrier`](crate::table::writer_drain_barrier)'s "F-70 —
    /// THE canonical lock-order hierarchy" module doc for the full 3-party
    /// derivation. Holding the lock is NOT required for `drain_writers()` to
    /// be correct: what matters is that the intent bit was raised first (so
    /// every NEW writer takes the slow path and cannot re-enter the drain
    /// set) — the lock only serializes those NEW slow-path writers against
    /// the caller's own body, a separate concern from the drain wait itself.
    ///
    /// Production code MUST go through [`begin_write_barrier`](Self::begin_write_barrier)
    /// (which performs the raise→drain→lock sequence in this exact order)
    /// instead of calling this method directly — `drain_writers()` is a raw
    /// primitive that `begin_write_barrier` (and the legacy direct callers
    /// that pre-date it) delegate to, not the sanctioned entry point. See
    /// [`writer_drain_barrier`](crate::table::writer_drain_barrier) for the
    /// full memory-model rationale and how F-50 reuses this same call.
    pub async fn drain_writers(&self) {
        self.writer_drain.drain().await;
    }

    /// F-48b (#867) — the writer-side twin of [`drain_writers`]: bump the
    /// drain-set counter and return an RAII guard that decrements on drop.
    ///
    /// Called by the tx-commit pipeline's Phase 2.5 prelock
    /// (`tx::pre_commit::pre_commit_prelock`) for EVERY table in `tx.write_set`
    /// BEFORE reading [`needs_write_barrier`](Self::needs_write_barrier) — the
    /// ordering is load-bearing (see [`writer_drain_barrier`] for the
    /// happens-before chain). If the flag is `true` (slow path), the caller
    /// drops the returned guard BEFORE taking [`unique_write_lock`] (the lock
    /// serializes the slow path; staying in the drain set while blocking on
    /// the lock would deadlock against a DDL holding the lock and waiting on
    /// [`drain_writers`]). If the flag is `false`, the caller keeps the guard
    /// alive until its Phase 5c materialize write has landed — so a DDL that
    /// raises the barrier AFTER the flag read genuinely waits for this tx.
    ///
    /// Visibility mirrors [`needs_write_barrier`] (`pub(crate)`): the only
    /// caller is the engine's own tx-commit prelock. The non-tx writer methods
    /// in `table_manager_crud.rs` access `self.writer_drain` directly (same
    /// module); this accessor exists for the cross-module prelock call site.
    pub(crate) fn enter_writer_drain(&self) -> super::writer_drain_barrier::WriterDrainGuard {
        self.writer_drain.enter_writer()
    }

    /// Borrow the table's sorted-index manager — used by the planner
    /// for range / order / min queries, and by DDL when a
    /// `create_index { sorted: true }` op lands.
    pub fn sorted_indexes(&self) -> &SortedIndexManager {
        &self.sorted_indexes
    }

    /// O(1) composite check: does this table have ANY index across all
    /// three subsystems (index2 registry, base_index hash/unique, sorted)?
    ///
    /// Used as a fast-path guard on the insert hot path to skip the
    /// `all_backends().await` scan + 3 base_index planner calls when the
    /// table has zero indexes. Each sub-check is O(1): `is_empty()`
    /// on `scc::HashMap`, an `AtomicBool` load, a packed-word bit test
    /// (F-69, #896), `DashMap::is_empty`.
    pub fn has_any_index(&self) -> bool {
        !self.index2_registry.is_empty()
            || self.index_manager.has_indexes()
            || self.index_manager.has_unique_indexes()
            || self.sorted_indexes.has_indexes()
    }

    /// O(1) predicate: must this writer acquire `unique_write_lock` before
    /// its validate→write→index sequence?
    ///
    /// `true` when ANY of:
    /// - the table has a base_index unique index (the original reason the barrier
    ///   exists — atomic unique-check + posting-write),
    /// - an index2 `create_index_v2` is currently in flight (#534 finding 1),
    ///   OR
    /// - a schema-activation DDL (`set_table_schema` / `add_schema_rule`) is
    ///   currently in its `keyset_safe` count-proof → persist → activate
    ///   window (F-37, #845), OR
    /// - a regular hash-index `create_index` (F-57, #883), unique-index
    ///   `create_unique_index` (F-57), or sorted-index
    ///   `create_sorted_index_with_include` (F-57) snapshot→backfill→register
    ///   sequence is currently in flight.
    ///
    /// F-69 (#896, P0): this is now EXACTLY ONE atomic load —
    /// `write_barrier_flags.any_set()` — instead of six independent loads
    /// (five `AtomicBool`s here plus `IndexManager::has_unique_indexes()`'s
    /// own, previously-`Relaxed`, flag). A single atomic load can never be
    /// torn regardless of memory ordering, so the six-condition compound is
    /// now a genuine point-in-time snapshot; see
    /// `crate::index::write_barrier_flags`'s module doc for the full
    /// before/after writeup and why this fixes a real duplicate-past-unique-
    /// constraint race. The load is `SeqCst` (F-56, #882): it is the writer
    /// half of a cross-atomic dependency with the `active` drain counter — a
    /// writer that reads `false` must have its prior `active.fetch_add`
    /// (also `SeqCst`) be observable by the drainer's `active.load` (also
    /// `SeqCst`). Only a single `SeqCst` total order across both atomics can
    /// carry that edge — `Release`/`Acquire` alone cannot (see
    /// [`writer_drain_barrier`]'s memory-model proof, which this task
    /// extends to state it now covers the FULL six-condition predicate).
    /// Tables with none of these conditions keep the lock-free fast path.
    ///
    /// Consulted by the non-tx writer methods in `table_manager_crud.rs`
    /// (`insert`/`insert_many_returning_version`/`delete_returning_version`/
    /// `set`), AND (task #538, Part A) by the tx-commit pipeline's Phase 2.5
    /// prelock (`tx::pre_commit::pre_commit_prelock`), which now acquires
    /// `unique_write_lock` for every table this tx wrote to that returns
    /// `true` here — not just tables with base_index unique guards. This closes
    /// the commit-time serialization gap for an index2-only table (no base_index
    /// unique index) under an in-flight `create_index_v2`. It does NOT close
    /// #538's Part B (stage-time index2 ops-plan staleness against a
    /// create that starts/finishes entirely between this tx's stage and
    /// commit) — see `TableManager::backfill_index2_backend`'s doc comment
    /// for the full accounting of what #534 and #538 each close and leave
    /// open.
    pub(crate) fn needs_write_barrier(&self) -> bool {
        self.write_barrier_flags.any_set()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Attach an MvccStore for tx-aware reads. Returns `self` so callers
    /// can chain after `create()`. When attached, `read_one_tx(rid, Some(tx))`
    /// reads through the MvccStore at `tx.snapshot_version`. Without an
    /// attached MvccStore, tx-aware reads fall through to the non-tx
    /// fast path (same as `get`).
    pub fn with_mvcc_store(mut self, mvcc: Arc<shamir_tx::MvccStore>) -> Self {
        self.mvcc_store = Some(mvcc);
        self
    }

    /// Stage I — replace this table's per-table [`InternerManager`] with the
    /// shared per-repo one. `RepoInstance::create_table_context` calls this
    /// so every table in a repo shares ONE live
    /// [`Interner`](shamir_types::core::interner::Interner) and id-namespace
    /// (a field name resolves to the SAME id across tables). Returns `self`
    /// for chaining after [`create`](Self::create), mirroring
    /// [`with_mvcc_store`](Self::with_mvcc_store).
    ///
    /// `InternerManager::clone` Arc-shares the live `Interner` (the
    /// `OnceCell<Interner>`, the chunk-persist atomics, and the persist
    /// mutex), so the per-table handle returned by [`interner`](Self::interner)
    /// is the SAME manager the repo owns — a write through any table's
    /// interner is visible to every other table's reads. The
    /// [`PersistRegistry`] keeps a separate clone of the per-table manager it
    /// was built with in [`create`](Self::create); that clone shares the same
    /// Arc state, so `flush_metadata` / `flush_buffers` persist the shared
    /// interner through any registered handle. Idempotent: re-attaching the
    /// same manager is a cheap clone.
    pub fn with_interner(mut self, interner: InternerManager) -> Self {
        self.interner = interner;
        self
    }

    /// Borrow the attached [`MvccStore`](shamir_tx::MvccStore), if any.
    /// Used by the index-only read path (slice A3) to validate covering-index
    /// posting freshness without fetching the record from the data store.
    pub(crate) fn mvcc_store_ref(&self) -> Option<&Arc<shamir_tx::MvccStore>> {
        self.mvcc_store.as_ref()
    }

    /// Public accessor for the attached MvccStore (the version-log handle).
    /// Used by the migration coordinator (Q1) to read the source snapshot
    /// through the log seam (`current_stream`) instead of the raw data_store.
    pub fn mvcc_store(&self) -> Option<Arc<shamir_tx::MvccStore>> {
        self.mvcc_store_ref().cloned()
    }

    /// Wire this table's non-tx write path to the SSI commit-write log.
    ///
    /// Returns `self` so callers can chain after `create()` (mirrors
    /// [`with_mvcc_store`](Self::with_mvcc_store)). `gate` MUST be the same
    /// per-repo [`RepoTxGate`](shamir_tx::RepoTxGate) the tx commit pipeline
    /// uses — that is what keeps non-tx and tx `commit_version`s on one
    /// monotonic sequence per repo, AND ensures non-tx writes are visible
    /// to Serializable transactions' Phase 2-bis predicate-conflict check.
    ///
    /// Attached by `RepoInstance::create_table_context`. When absent, the
    /// non-tx write methods skip SSI footprint recording entirely (system
    /// tables / direct-constructed test tables).
    pub fn with_changefeed(mut self, gate: Arc<shamir_tx::RepoTxGate>) -> Self {
        self.changefeed = Some(NonTxChangefeed { gate });
        self
    }
}

/// F-70 (#897, P0) — RAII guard returned by
/// [`TableManager::begin_write_barrier`]: raises one bit of the shared
/// [`WriteBarrierFlags`](crate::index::write_barrier_flags::WriteBarrierFlags)
/// word on construction, clears it on drop (including every `?` early-return
/// path in the caller). Owns a CLONE of the `Arc<AtomicU8>`-backed flags
/// word (cheap — one refcount bump) rather than borrowing it, so it can be
/// returned by value from `begin_write_barrier` with no lifetime tied to
/// `&TableManager` — the shape a cross-crate `pub` acquisition helper needs.
///
/// F-95 (#957, P0): this guard ALSO embeds the per-table DDL admission
/// mutex guard (`ddl_admission`), acquired BEFORE the bit is raised inside
/// [`begin_write_barrier`](TableManager::begin_write_barrier). Embedding it
/// here — rather than returning it as a third tuple element — means every
/// existing call site's destructuring (`let (_barrier, _lock) = …`) is
/// unchanged, and the admission guard's lifetime is automatically tied to
/// this guard's. Drop order is correct: the manual `Drop::drop` clears the
/// bit first, then Rust drops the struct's fields in declaration order
/// (`flags` → `bit` → `admission`), so the admission mutex is released
/// ONLY after the bit has been cleared — the next DDL waiting on admission
/// wakes to a clean state (bit 0) and does its own raise→drain→lock from
/// scratch.
///
/// Supersedes the old per-module `IndexCreateBarrierGuard`
/// (`table_manager_index_mgmt.rs`) and `SchemaActivationBarrierGuard`
/// (`shamir-db::execute::admin_schema`), both of which independently raised
/// a bit under an ALREADY-HELD `unique_write_lock` (lock-then-drain — the
/// F-70 inversion). This guard is constructed BEFORE the lock is taken (see
/// [`begin_write_barrier`](TableManager::begin_write_barrier)); the bit is
/// still `SeqCst`-ordered (F-56, extended by F-69 to the whole word), which
/// is what the [`writer_drain_barrier`] proof requires regardless of when
/// the lock is acquired relative to the drain.
#[must_use]
pub struct WriteBarrierGuard {
    flags: crate::index::write_barrier_flags::WriteBarrierFlags,
    bit: u8,
    /// F-95 (#957): the DDL admission mutex guard, held for the caller's
    /// entire critical section. Declared LAST so it drops AFTER the
    /// manual `Drop::drop` has already cleared `bit`.
    // The field is never *read* — it is held purely for its `Drop`
    // side-effect (releasing `ddl_admission` after the bit is cleared).
    // Without it the admission mutex would be released at the end of
    // `begin_write_barrier` rather than the end of the caller's critical
    // section, reopening the exact multi-owner race this guard closes.
    #[allow(dead_code)]
    admission: tokio::sync::OwnedMutexGuard<()>,
}

impl WriteBarrierGuard {
    fn set(
        flags: crate::index::write_barrier_flags::WriteBarrierFlags,
        bit: u8,
        admission: tokio::sync::OwnedMutexGuard<()>,
    ) -> Self {
        flags.set(bit);
        Self {
            flags,
            bit,
            admission,
        }
    }
}

impl Drop for WriteBarrierGuard {
    fn drop(&mut self) {
        self.flags.clear(self.bit);
    }
}

/// Parse a DSL tokenizer spec string into a [`TokenizerKind`].
///
/// DSL names:
///   - `None` / `"whitespace"` / unknown → `Whitespace`
///   - `"unicode"` → `Unicode`
///   - `"ngram"` → `Ngram { n: 3 }` (default trigram)
///   - `"ngram2"` .. `"ngram9"` → `Ngram { n: <digit> }`
///   - `"stemmed_<lang>"` → `Full { <lang>, stopwords=true, stem=true }`
///     (falls back to `Whitespace` if the language suffix is unknown)
pub(crate) fn fts_tokenizer_from_dsl(spec: Option<&str>) -> crate::index2::kind::TokenizerKind {
    use crate::index2::kind::{StemLanguage, TokenizerKind};

    match spec {
        Some("unicode") => TokenizerKind::Unicode,
        Some("ngram") => TokenizerKind::Ngram { n: 3 },
        Some(s) if s.starts_with("ngram") => {
            let digits = &s["ngram".len()..];
            let n: u8 = digits.parse().unwrap_or(3);
            TokenizerKind::Ngram { n: n.max(1) }
        }
        Some(s) if s.starts_with("stemmed_") => {
            let rest = &s["stemmed_".len()..];
            match StemLanguage::from_dsl(rest) {
                Some(lang) => TokenizerKind::Full {
                    language: lang,
                    stopwords: true,
                    stem: true,
                },
                None => TokenizerKind::Whitespace,
            }
        }
        _ => TokenizerKind::Whitespace,
    }
}
