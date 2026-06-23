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
    pub(super) index2_registry: Arc<crate::index2::IndexRegistry>,
    pub(super) mvcc_store: Option<Arc<shamir_tx::MvccStore>>,
    /// Per-table validator bindings (S2). Lock-free reads via
    /// `ArcSwap`; the S3 write path reads this on every write.
    /// DDL (`add_validator_binding` / `remove_validator_binding`)
    /// mutates + persists to the info-twin.
    pub(super) validator_bindings: Arc<arc_swap::ArcSwap<Vec<crate::validator::ValidatorBinding>>>,
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
            index2_registry: Arc::clone(&self.index2_registry),
            mvcc_store: self.mvcc_store.clone(),
            validator_bindings: Arc::clone(&self.validator_bindings),
            validator_registry: self.validator_registry.clone(),
            changefeed: self.changefeed.clone(),
            scalar_resolver: Arc::clone(&self.scalar_resolver),
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
        let validator_bindings =
            match crate::validator::persistence::load_validators_metadata(&info_store).await? {
                Some(pv) => Arc::new(arc_swap::ArcSwap::from_pointee(pv.bindings)),
                None => Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())),
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
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
            validator_bindings,
            validator_registry: None,
            changefeed: None,
            scalar_resolver: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                shamir_funclib::scalar_resolver::ScalarResolver::builtins_only(),
            ))),
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
        if let Some(persisted) =
            crate::index2::persistence::load_index2_metadata(&mgr.info_store).await?
        {
            mgr.index2_registry.set_next_id(persisted.next_id);
            for desc in persisted.descriptors {
                if matches!(desc.kind, crate::index2::kind::IndexKind::Btree { .. }) {
                    continue;
                }
                let backend = crate::index2::build_index2_backend_with_resolver(
                    desc,
                    &info_store,
                    Some(mgr.scalar_resolver.load_full().as_ref().clone()),
                );
                let _ = mgr.index2_registry.insert(backend).await;
            }
        }

        // Rebuild in-memory state from persisted data.
        // Vector backends lose their HNSW graph; FTS ranked backends
        // lose BM25 doc_count/sum_doc_len counters; others are no-op.
        {
            let backends = mgr.index2_registry.all_backends().await;
            for b in &backends {
                let data = Arc::clone(mgr.table.data_store());
                if let Err(e) = b.rebuild(data).await {
                    log::warn!(
                        "index2 rebuild failed for index {}: {}",
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
            index2_registry: Arc::new(crate::index2::IndexRegistry::new()),
            mvcc_store: None,
            validator_bindings: Arc::new(arc_swap::ArcSwap::from_pointee(Vec::new())),
            validator_registry: None,
            changefeed: None,
            scalar_resolver: Arc::new(arc_swap::ArcSwap::new(std::sync::Arc::new(
                shamir_funclib::scalar_resolver::ScalarResolver::builtins_only(),
            ))),
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

    /// Clone the handle to this table's unique-write serialisation lock.
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
