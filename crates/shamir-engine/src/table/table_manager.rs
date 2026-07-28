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
    /// Set to `true` (under `unique_write_lock`) for the duration of an
    /// index2 (`fts`/`functional`/`vector`) `create_index_v2` backfill →
    /// register sequence. While it is `true`, EVERY writer path also
    /// acquires `unique_write_lock`, so no row can be written in the
    /// backfill→register window where it would be seen by NEITHER the
    /// backfill snapshot (cursor already past it) NOR the live
    /// `index2_on_insert` hook (backend not yet registered) — the
    /// lost-write race (#534, finding 1) that `unique_write_lock` alone
    /// does not close because writers only take that lock when a legacy
    /// unique index exists (`has_unique_indexes()`), and an index2-only
    /// table has none. Shared across clones via `Arc` so a create on any
    /// clone is observed by writers on every clone (same rationale as
    /// `bindings_len`). Loaded `Acquire` on the writer fast-path skip.
    pub(super) index2_create_barrier: Arc<std::sync::atomic::AtomicBool>,
    /// F-37 (#845) — sibling of `index2_create_barrier`, raised for the
    /// duration of a schema-activation DDL sequence (`set_table_schema` /
    /// `add_schema_rule`) whose `keyset_safe` proof reads
    /// `table.count() == 0` and then persists + activates a new schema rule.
    /// While it is `true`, EVERY writer path also acquires
    /// `unique_write_lock`, so no row can land between the count proof and
    /// the schema's activation — the F-37 race that `lock_schema_rmw` does
    /// NOT close (that lock only serializes schema DDL against OTHER schema
    /// DDL in `admin_user_locks`, never against the write path in
    /// `table_manager_crud.rs`). Sibling, NOT overload: `index2_create_barrier`
    /// and this flag represent different in-flight conditions and are
    /// independently settable/clearable. Shared across clones via `Arc`
    /// (same rationale as `index2_create_barrier`); loaded `Acquire` on the
    /// writer fast-path skip via [`needs_write_barrier`](Self::needs_write_barrier).
    /// Set/cleared `Release` (under `unique_write_lock`) by the shamir-db DDL
    /// handler via [`set_schema_activation_barrier`](Self::set_schema_activation_barrier).
    pub(super) schema_activation_barrier: Arc<std::sync::atomic::AtomicBool>,
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
            // Shared across clones so a `create_index_v2` in flight on any
            // clone forces writers on every clone onto the barrier.
            index2_create_barrier: Arc::clone(&self.index2_create_barrier),
            // F-37 — shared across clones so a schema-activation DDL in flight
            // on any clone forces writers on every clone onto the barrier.
            schema_activation_barrier: Arc::clone(&self.schema_activation_barrier),
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
            index2_create_barrier: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            schema_activation_barrier: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            index2_create_barrier: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            schema_activation_barrier: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    /// F-37 (#845) — drive the schema-activation write barrier from outside
    /// the engine crate (the shamir-db schema DDL handler in
    /// `admin_schema.rs`). When `on == true`, every writer consulting
    /// [`needs_write_barrier`](Self::needs_write_barrier) returns `true` and
    /// serializes on [`unique_write_lock`](Self::unique_write_lock) — closing
    /// the `keyset_safe` count-proof race (a concurrent INSERT/UPDATE can no
    /// longer land between the `count() == 0` read and the schema's persist +
    /// activate).
    ///
    /// `Release`-ordered to pair with the writer's `Acquire` load in
    /// `needs_write_barrier`. **Callers MUST set/clear this while holding
    /// `unique_write_lock`** (mirrors `index2_create_barrier`'s discipline in
    /// `create_index_v2`): raise the flag under the lock so the
    /// `count() == 0` proof that follows is a genuine snapshot no concurrent
    /// writer can invalidate, and clear it (still under the lock) once
    /// persist + activate have committed — the lock is then released, letting
    /// queued writers proceed ordered AFTER the proof point. An RAII guard
    /// that clears on drop (every exit path) is the sanctioned usage shape —
    /// see `admin_schema.rs::SchemaActivationBarrierGuard`.
    pub fn set_schema_activation_barrier(&self, on: bool) {
        self.schema_activation_barrier
            .store(on, std::sync::atomic::Ordering::Release);
    }

    /// F-48 (#859, P0) — drain every in-flight fast-path writer before the
    /// caller proceeds past a snapshot/proof point (the `keyset_safe`
    /// count-proof for schema activation; the backfill snapshot for index2
    /// create in F-50).
    ///
    /// The caller MUST have already (1) raised its intent flag
    /// (`schema_activation_barrier` / `index2_create_barrier`) so NEW writers
    /// take the slow (locked) path, and (2) hold `unique_write_lock` so
    /// slow-path writers are blocked. Then this catches any writer that read
    /// `false` before the flag went up and is still in its
    /// validate→write→index sequence. Returns immediately (one `Acquire`
    /// load) when no fast-path writer is active.
    ///
    /// See [`writer_drain_barrier`](crate::table::writer_drain_barrier) for
    /// the full memory-model rationale and how F-50 reuses this same call.
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
    /// three subsystems (index2 registry, legacy hash/unique, sorted)?
    ///
    /// Used as a fast-path guard on the insert hot path to skip the
    /// `all_backends().await` scan + 3 legacy planner calls when the
    /// table has zero indexes. Each sub-check is O(1): `is_empty()`
    /// on `scc::HashMap`, two `AtomicBool` loads, `DashMap::is_empty`.
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
    /// - the table has a legacy unique index (the original reason the barrier
    ///   exists — atomic unique-check + posting-write),
    /// - an index2 `create_index_v2` is currently in flight (#534 finding 1),
    ///   OR
    /// - a schema-activation DDL (`set_table_schema` / `add_schema_rule`) is
    ///   currently in its `keyset_safe` count-proof → persist → activate
    ///   window (F-37, #845).
    ///
    /// Each barrier flag is loaded `Acquire` to pair with the `Release` store
    /// the corresponding create/DDL path makes under the lock. Tables with
    /// none of these conditions keep the lock-free fast path.
    ///
    /// Consulted by the non-tx writer methods in `table_manager_crud.rs`
    /// (`insert`/`insert_many_returning_version`/`delete_returning_version`/
    /// `set`), AND (task #538, Part A) by the tx-commit pipeline's Phase 2.5
    /// prelock (`tx::pre_commit::pre_commit_prelock`), which now acquires
    /// `unique_write_lock` for every table this tx wrote to that returns
    /// `true` here — not just tables with legacy unique guards. This closes
    /// the commit-time serialization gap for an index2-only table (no legacy
    /// unique index) under an in-flight `create_index_v2`. It does NOT close
    /// #538's Part B (stage-time index2 ops-plan staleness against a
    /// create that starts/finishes entirely between this tx's stage and
    /// commit) — see `TableManager::backfill_index2_backend`'s doc comment
    /// for the full accounting of what #534 and #538 each close and leave
    /// open.
    pub(crate) fn needs_write_barrier(&self) -> bool {
        self.index_manager.has_unique_indexes()
            || self
                .index2_create_barrier
                .load(std::sync::atomic::Ordering::Acquire)
            || self
                .schema_activation_barrier
                .load(std::sync::atomic::Ordering::Acquire)
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
