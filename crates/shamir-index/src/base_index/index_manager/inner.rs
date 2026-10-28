use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::write_barrier_flags::WriteBarrierFlags;
use bytes::Bytes;
use dashmap::DashMap;
use shamir_storage::types::Store;
use shamir_tx::{IndexFamily, Provenance};
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type PostingCache = DashMap<Bytes, (u64, Arc<[RecordId]>), THasher>;

/// R0-B (#1008): [`Provenance`] for an op planned against `def` on the
/// REGULAR (non-unique) base_index family. See
/// `IndexDefinition::provenance`'s doc.
pub(super) fn regular_provenance(def: &IndexDefinition) -> Provenance {
    def.provenance(IndexFamily::Regular)
}

/// Log CREATE INDEX backfill progress at most this often (avoids spamming the
/// log on every batch of a large table scan).
pub(super) const BACKFILL_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum number of posting-list entries cached in memory per
/// `IndexManager`. Hit on a cached entry is a single `HashMap::get`
/// + `Arc::clone`; miss falls back to `info_store.get` + bincode
///   deserialise. Capacity is intentionally small — typical workloads
///   (admin UIs, filter-by-status, find-by-id) concentrate on a handful
///   of values per index.
pub(super) const POSTING_CACHE_CAP: usize = 512;

/// #997: durable RENAME INDEX tombstone payload for the base_index REGULAR
/// and UNIQUE (hash) families. Unlike the sorted family's
/// `old_id → new_id` rekey tombstone (#962, which only needs the id pair
/// because a sorted rename is an in-place rekey of postings that survive
/// under either id), a hash rename is a **drop + rebuild**: the hash
/// physical key mixes `name_interned` into h1/h2 (`compute_leaf_hashes` /
/// `compute_lookup_hashes`), so the OLD postings cannot be rekeyed — they
/// must be swept and the index re-backfilled under the NEW name. Worse, the
/// UNIQUE path drops the OLD definition FIRST (`drop_unique_index` before
/// `create_unique_index_body`), so by the time a crash can strand this
/// tombstone the old `IndexDefinition` — and therefore its `paths` — is
/// already GONE from both memory and disk. This payload therefore MUST
/// carry enough to rebuild from nothing.
///
/// Fields:
/// - `old_name` / `new_name`: the resolved STRING names (NOT interned ids).
///   Recovery re-interns them via the (reloaded) interner rather than
///   trusting a stored u64 id survived the crash's interner persist — this
///   is robust to a crash that stranded the tombstone BEFORE
///   `create_index`'s F-42 interner persist ran, where `new_name`'s id
///   might not even be durably interned yet. `old_name` is also what the
///   rename was issued under, so it is always already interned.
/// - `paths`: the resolved dot-separated STRING paths
///   (`resolve_index_paths` direction). Strings (not the interned
///   `Vec<IndexInfoItem>`) for the same robustness reason, AND because the
///   rebuild calls `create_index` / `create_unique_index_body`, which take
///   `&[&str]` — strings are needed at the call site regardless.
/// - `op_id`: the DDL operation ID minted at dispatch time (#1015). Used by
///   recovery to write `SucceededViaCrashRecovery` to the op-status log.
///
/// Persisted under `system:idx_ren` (regular) / `system:uidx_ren` (unique)
/// as `Vec<HashRenameTombstone>` via bincode. Mirrors #959's
/// `idx_drop`/`uidx_drop` (two keys, one per family) and #962's
/// `sidx_ren`. See `IndexManager`'s `renaming_regular`/`renaming_unique`
/// fields and `TableManager::recover_hash_renames`'s crash-state matrix.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HashRenameTombstone {
    /// The index name being renamed FROM (resolved string).
    pub old_name: String,
    /// The index name being renamed TO (resolved string).
    pub new_name: String,
    /// The index's field paths as resolved dot-separated strings
    /// (e.g. `["user.email"]`), sufficient to rebuild the index.
    pub paths: Vec<String>,
    /// The DDL operation ID minted at dispatch time (#1015).
    /// Used by recovery to write `SucceededViaCrashRecovery` to the op-status log.
    pub op_id: Option<String>,
}

/// #1051: tombstone entry for in-progress DROP INDEX operations on the
/// hash families (regular and unique). Carries the `name_interned` of the
/// index being dropped and the operation ID minted at dispatch time.
///
/// Persisted under `system:idx_drop` (regular) / `system:uidx_drop` (unique)
/// as `Vec<HashDropTombstone>` via bincode. An absent key or empty vec means
/// "no in-progress drops".
///
/// The `op_id` field enables recovery to write `SucceededViaCrashRecovery`
/// to the op-status log with the correct operation ID, ensuring clients can
/// poll the exact operation they triggered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HashDropTombstone {
    /// The `name_interned` of the index being dropped.
    pub name_interned: u64,
    /// The DDL operation ID minted at dispatch time (#1015 / #1051).
    /// Used by recovery to write `SucceededViaCrashRecovery` to the op-status log.
    /// `None` for pre-#1051 tombstones (backward compatibility).
    pub op_id: Option<String>,
}

/// Менеджер индексов для одной таблицы.
///
/// Инкапсулирует всю логику работы с индексами:
/// - Хранит метаданные индексов в памяти
/// - Синхронизирует изменения с диском
/// - Обновляет индексы при изменении данных
///
/// # Clone semantics
///
/// Клонирование создаёт shared reference на те же данные.
/// Все клоны видят изменения друг друга.
pub struct IndexManager {
    /// Хранилище данных таблицы (для чтения записей при построении индекса)
    pub(in crate::base_index) data_store: Arc<dyn Store>,
    /// Служебное хранилище для метаданных и записей индексов
    pub(in crate::base_index) info_store: Arc<dyn Store>,

    /// Метаданные обычных индексов (неуникальных)
    /// IndexInfo использует DashMap внутри, поэтому thread-safe без дополнительной синхронизации
    pub(in crate::base_index) indexes: Arc<IndexInfo>,
    /// Метаданные уникальных индексов
    pub(in crate::base_index) indexes_unique: Arc<IndexInfo>,

    /// F-69 (#896, P0): the packed word `TableManager` (`shamir-engine`)
    /// shares as the single atomic backing `needs_write_barrier()`. Replaces
    /// the former standalone `has_indexes: Arc<AtomicBool>` (written
    /// `Release`, read `Relaxed` — no happens-before edge; see #1102 brief)
    /// and `has_indexes_unique: Arc<AtomicBool>` (loaded `Relaxed`, the
    /// torn-read bug F-69 closes). This manager is the exclusive write
    /// authority for bits 0 (`UNIQUE_INDEX_EXISTS`) and 6 (`REGULAR_INDEX_EXISTS`);
    /// `TableManager` clones the same `Arc<AtomicU16>` and owns bits 1-5. The
    /// module doc for `write_barrier_flags.rs` explains why bits 3 and 4 are
    /// "CREATE in flight" flags, distinct from the EXISTS flags.
    pub(in crate::base_index) write_barrier_flags: WriteBarrierFlags,

    /// **Opt G** — in-memory cache for posting lists. Keys are the
    /// raw physical index keys (`build_index_key(...).to_bytes()`);
    /// values are `Arc<[RecordId]>` — a sorted, duplicate-free,
    /// immutable slice (audit 3.2 / task #499). Shared between hits so
    /// the lookup hot path is `HashMap::get` + `Arc::clone` (O(1)); the
    /// consumer then iterates a contiguous, cache-friendly buffer
    /// instead of chasing `BTreeSet` tree-node pointers.
    ///
    /// Cache entries carry the generation at scan time. Writes advance the
    /// shared epoch so even a missed per-key removal cannot serve stale data.
    /// arbitrary entry (exact LRU not worth the dep here; index
    /// hotsets are typically small).
    ///
    /// `DashMap` replaces the previous `Mutex<HashMap>` so concurrent
    /// readers never serialise through a single lock — each shard
    /// is independently lockable and the read path on a cache hit is
    /// fully lock-free against unrelated index keys. Cache hits on
    /// the same shard still take the per-shard read lock.
    pub(in crate::base_index) posting_cache: Arc<PostingCache>,

    /// Monotonic cache invalidation epoch, shared by all manager clones.
    pub(in crate::base_index) posting_cache_generation: Arc<AtomicU64>,

    /// F-72 (#899, P0) test-only deterministic pause point: parks
    /// `create_index_from_records`'s Phase 2 backfill loop (definition
    /// already registered at `state = Building`, hence planner-invisible)
    /// so a regression test can drive a concurrent read into the exact
    /// window this task closes. `None` on every real path — a cheap
    /// `ArcSwapOption::load_full()` (uncontended `Acquire` load) at the one
    /// call site, no lock, no allocation. NOT `#[cfg(test)]`-gated: the
    /// consuming test (`shamir-engine`'s `f72_planner_invisibility_tests.rs`)
    /// installs this hook from a DIFFERENT crate's test binary, where THIS
    /// crate's own `cfg(test)` is inactive — see `base_index/mod.rs`'s module
    /// doc on `backfill_pause_hook` for the cross-crate-visibility reason.
    pub(in crate::base_index) create_index_backfill_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    /// F-76 (#903) test-only deterministic pause point: parks `drop_index`
    /// (regular) AND `drop_unique_index` (unique) between the definition
    /// retirement and the posting sweep — the exact visibility window this
    /// task closes (the mirror image of F-72's CREATE bug). A regression
    /// test installs this from `shamir-engine`'s test binary, so (like
    /// `create_index_backfill_hook`) it is NOT `#[cfg(test)]`-gated. Shared
    /// by both base_index DROP paths — a test exercises only one at a time.
    /// `None` on every real path; one uncontended `ArcSwapOption::load_full()`
    /// Acquire load at the call site. See `f76_drop_visibility_tests.rs`.
    pub(in crate::base_index) drop_index_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-3a (#1011): test-only deterministic pause point INSIDE
    /// `lookup_by_index`, fired AFTER the [`reader_gate`](Self::reader_gate)
    /// guard is acquired and BEFORE the `info_store` scan loop begins — so a
    /// test can park a read mid-flight WHILE it holds the gate's in-flight
    /// counter, deterministically proving a concurrent `drop_index`'s drain
    /// wait blocks until that read releases the guard. NOT `#[cfg(test)]`-gated
    /// — cross-crate test consumer (`shamir-engine`'s `p1011_reader_drain_tests`
    /// reaches it via `index_manager_ref()`), the SAME rationale documented on
    /// `drop_index_pause_hook` above. `None` on every real path; one
    /// uncontended `ArcSwapOption::load_full()` Acquire load at the call site.
    pub(in crate::base_index) lookup_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// D3ED367D test seam: pause after the posting scan and before cache publication.
    /// NOT `#[cfg(test)]`-gated for cross-crate regression consumers.
    pub(in crate::base_index) lookup_post_scan_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-2 (#958): monotonic generation counter bumped whenever the set of
    /// base_index index definitions (regular OR unique) changes — i.e. on every
    /// `create_index` / `create_index_from_records` / `create_index_from_stream`
    /// / `create_unique_index[_from_records]` / `drop_index` /
    /// `drop_unique_index`. Read with [`generation`](Self::generation) to gate
    /// commit-time ops-plan re-derivation, exactly like `IndexRegistry`'s
    /// generation does for index2 backends and `SortedIndexManager`'s does for
    /// sorted indexes. A tx that staged BEFORE a base_index index was created
    /// would otherwise commit with zero ops for the new index — a
    /// permanently missing posting (for regular) or a permanently
    /// unconstrained duplicate (for unique). The commit-time gate re-plans
    /// ops for the new defs and, for unique defs, records fresh
    /// `UniqueGuard`s so the existing Phase 2.6 re-validation covers them.
    pub(in crate::base_index) generation: Arc<AtomicU64>,

    /// P0-3 (#959 / #1051): in-memory mirror of the persisted tombstone set for
    /// in-progress base_index DROP INDEX operations (regular family). When
    /// `drop_index` starts, it adds `name_interned` here AND persists it
    /// durably (see `add_to_dropping`) BEFORE sweeping postings; when the
    /// drop completes (sweep + persist reduced IndexInfo), it removes the
    /// entry (see `clear_from_dropping`). On restart, `IndexManager::new`
    /// loads the persisted tombstone, runs the recovery sweep, and clears
    /// both the persisted and in-memory sets. Also consulted by
    /// `create_index[_from_records][_from_stream]` to reject a CREATE that
    /// would reuse a name whose DROP is still in flight (sub-bug 3b).
    ///
    /// Keyed by `name_interned`; the value is the `op_id` minted at dispatch
    /// time (#1051) for crash recovery status writes.
    ///
    /// `std::sync::Mutex` is the sanctioned low-frequency fallback here
    /// (CLAUDE.md: "only low-frequency/setup fallbacks, justified inline").
    /// DROP INDEX is a DDL operation — contention is nil in normal
    /// operation and the set is empty 99.999 % of the time. A `DashMap`
    /// would be unjustified overkill for a guard that fires once per DDL.
    /// The lock is NEVER held across an `.await` point.
    pub(in crate::base_index) dropping_regular: Arc<Mutex<BTreeMap<u64, Option<String>>>>,
    /// P0-3 (#959 / #1051): same as `dropping_regular`, for the unique family.
    pub(in crate::base_index) dropping_unique: Arc<Mutex<BTreeMap<u64, Option<String>>>>,

    /// #1058: in-flight online build registry for the regular (hash) family.
    /// Tracks which indexes are currently in Phase B/C/D (barrier-free,
    /// dirty-set-capturing). Keyed by `name_interned`; presence indicates an
    /// online build is in flight for that index.
    ///
    /// Uses `scc::HashMap` (lock-free, CAS-based, sharded) per this repo's
    /// NORMATIVE concurrency invariants (CLAUDE.md pillar 5, "shared registry"
    /// row). The `is_build_in_flight()` method is called from inside
    /// `plan_record_created`/`plan_record_updated`/`plan_record_deleted`,
    /// which are the shared planning methods every insert/update/delete funnels
    /// through (per the write-path audit). This is a hot-path check, so
    /// lock-free is required — using a blocking `std::sync::Mutex` here would
    /// serialize all writes on any table with a `Building` index, violating
    /// the repo's stated idiom.
    ///
    /// The brief (#1059) owns the lifecycle — registration at Phase B start,
    /// removal at Phase D completion or abort. This slice (1c) only provides
    /// the data structure and API, not the lifecycle wiring.
    pub(in crate::base_index) in_flight_builds: Arc<scc::HashMap<u64, (), THasher>>,

    /// #1058: in-memory dirty-set for in-flight online builds (regular family).
    /// Keyed by `name_interned`; each value is a set of `RecordId`s touched
    /// by writes while that index's online build is in flight. Phase C reads
    /// each dirty-set and re-indexes the affected records. This is IN-MEMORY
    /// only — per RFC v2 §4.2, a crash loses the dirty-set, but slice 1's
    /// conservative crash-recovery policy is restart-from-scratch (a crash
    /// also restarts Phase A from scratch per #1060's design), so nothing needs
    /// the set to survive a crash in slice 1. The dirty-set is never persisted;
    /// resumable Phase-C recovery is explicitly deferred to slice 2+.
    ///
    /// Uses `Arc<Mutex<BTreeMap<u64, Arc<Mutex<BTreeSet<RecordId>>>>>>`:
    /// - `BTreeSet` provides sorted, duplicate-free RecordId storage (cheap
    ///   iteration for Phase C drain, dedup by construction).
    /// - `Mutex` protects concurrent modifications — acceptable here because
    ///   this is only touched AFTER `is_build_in_flight()` has already
    ///   returned true (i.e., only for indexes genuinely mid-online-build,
    ///   a legitimately rare/low-frequency case). The lock is NEVER held
    ///   across an `.await` point.
    ///
    /// TODO: Convert to `scc::HashMap` for full lock-free consistency with
    /// `in_flight_builds`. This is deferred because the hot-path check lives
    /// in `is_build_in_flight` (now lock-free), and `dirty_sets` is only
    /// accessed after that check passes — the blocking cost is bounded to the
    /// number of concurrent writes to actively-building indexes (near-zero
    /// in normal operation). A future cleanup can migrate this to scc for
    /// uniformity.
    ///
    /// Registry/dirty-set lifecycle (owned by #1059, not this slice):
    /// - Phase B start: `mark_build_in_flight(name_interned)` creates the
    ///   registry entry.
    /// - During Phase B/C: writes touching the building index add `RecordId`s
    ///   to its dirty-set (via the planning methods' dirty-set capture logic).
    /// - Phase D completion: `clear_build_in_flight(name_interned)` removes the
    ///   entry, Phase C drains the dirty-set before this happens.
    #[allow(clippy::type_complexity)]
    pub(in crate::base_index) dirty_sets: Arc<Mutex<BTreeMap<u64, Arc<Mutex<BTreeSet<RecordId>>>>>>,

    /// P0-3 (#959): test-only deterministic pause point that fires AFTER
    /// the posting sweep but BEFORE the reduced `IndexInfo` is persisted —
    /// the exact crash window sub-bug 3c tests. A regression test installs
    /// this hook, parks `drop_index` / `drop_unique_index` mid-drop, drops
    /// the manager (simulating a crash), then constructs a fresh
    /// `IndexManager::new` against the SAME `info_store` and asserts the
    /// recovery path finishes the drop instead of resurrecting a broken
    /// `Ready` index. NOT `#[cfg(test)]`-gated — cross-crate test consumer,
    /// same reason as `drop_index_pause_hook`. `None` on every real path.
    pub(in crate::base_index) drop_index_post_sweep_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// #1069 test-only deterministic pause point: parks `drop_index` (regular)
    /// AND `drop_unique_index` (unique) AFTER the terminal `Succeeded` status
    /// is written but BEFORE `clear_from_dropping` runs — the exact crash
    /// window the inline-path write-order fix closes. A regression test
    /// installs this hook, parks the drop at that point, then reads the
    /// op-status log via `ddl_op_log::read_op_status` and asserts it already
    /// shows `Succeeded`, proving the status write is durable before the
    /// tombstone clear could crash and lose it. NOT `#[cfg(test)]`-gated —
    /// cross-crate test consumer, same reason as `drop_index_pause_hook`.
    /// `None` on every real path.
    pub(in crate::base_index) drop_index_status_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// #997: in-memory mirror of the persisted "Renaming" tombstone for
    /// in-progress RENAME INDEX operations on the REGULAR (hash,
    /// `is_unique=0`) family. Keyed by `old_name_interned`; the value
    /// carries the resolved string name + paths recovery needs to rebuild
    /// from nothing (unlike sorted's `old_id → new_id` rekey tombstone, a
    /// hash rename is a drop+rebuild whose OLD definition is GONE —
    /// durably dropped FIRST in the unique path, and dropped second but
    /// unrecoverable-from-postings in the regular path — by the time a
    /// crash can strand the tombstone). `TableManager::rename_index` writes
    /// the tombstone BEFORE the first mutating step and clears it AFTER
    /// the last; on restart `TableManager::recover_hash_renames`
    /// (engine-side — it needs the record stream + interner for a
    /// backfill) finishes any interrupted rename. Mirrors #962's
    /// `renaming_sorted` (structurally) and #959's `dropping_regular`
    /// (same `IndexManager` owner) — see `HashRenameTombstone`'s doc and
    /// `recover_hash_renames`'s crash-state matrix for the full reasoning.
    ///
    /// `std::sync::Mutex` is the sanctioned low-frequency fallback here
    /// (CLAUDE.md): RENAME INDEX is a DDL op, contention is nil, the set
    /// is empty 99.999 % of the time, and the lock is NEVER held across
    /// an `.await` point. Mirrors `dropping_regular`.
    pub(in crate::base_index) renaming_regular: Arc<Mutex<BTreeMap<u64, HashRenameTombstone>>>,
    /// #997: same as `renaming_regular`, for the UNIQUE (hash,
    /// `is_unique=1`) family. The unique path is the SEVERE crash case
    /// (drop-OLD-first → create-NEW), so the tombstone's stored `paths`
    /// are the ONLY way to rebuild the constraint after a crash.
    pub(in crate::base_index) renaming_unique: Arc<Mutex<BTreeMap<u64, HashRenameTombstone>>>,

    /// #997: test-only deterministic pause point that fires in
    /// `TableManager::rename_index`'s regular+unique paths AFTER the
    /// tombstone is written but BEFORE the second mutating step
    /// (`drop_index` for regular, `create_unique_index_body` for unique) —
    /// the exact mid-rename crash window. A regression test installs this
    /// from `shamir-engine`'s test binary, so (like `drop_index_pause_hook`)
    /// it is NOT `#[cfg(test)]`-gated. `None` on every real path; one
    /// uncontended `ArcSwapOption::load_full()` Acquire load at the call
    /// site. Mirrors #962's `rename_rekey_pause_hook`.
    pub(in crate::base_index) rename_mid_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// #997 post-review fix (task #1000): test-only deterministic pause
    /// point that fires in `TableManager::recover_hash_renames`'s
    /// per-family loop, AFTER one tombstone entry has been fully
    /// reconciled but BEFORE the next entry (or the final
    /// `clear_all_renaming`) is processed. A regression test installs this
    /// hook to simulate recovery being interrupted between two stranded
    /// tombstone entries — the exact window that exposed the bug where a
    /// per-entry `clear_from_renaming` call (deriving its snapshot from the
    /// always-empty-at-open-time `renaming_regular`/`renaming_unique` maps)
    /// silently discarded a not-yet-processed sibling entry's tombstone.
    /// NOT `#[cfg(test)]`-gated — cross-crate test consumer, same reason as
    /// `rename_mid_pause_hook`. `None` on every real path.
    pub(in crate::base_index) recover_renames_between_entries_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,

    /// P0-3a (#1011): reader-vs-DROP mutual exclusion for the REGULAR
    /// (non-unique hash) family only — see `crate::reader_drain_gate`'s
    /// module doc for the full design. `lookup_by_index` (the sole
    /// production read chokepoint) enters this gate for the duration of
    /// its `info_store` scan; `drop_index` raises it before the RCU
    /// retire and drains it before the posting sweep. The UNIQUE family
    /// deliberately does NOT get its own instance of this gate — see
    /// `check_unique_key`'s doc comment for why its production reads are
    /// already serialized by `unique_write_lock`/`drain_writers` and would
    /// gain nothing from one.
    pub(in crate::base_index) reader_gate: crate::reader_drain_gate::ReaderDrainGate,
    /// #1102 test-only deterministic pause point: fires in all four regular
    /// index CREATE paths strictly AFTER the `REGULAR_INDEX_EXISTS` flag is
    /// set and BEFORE the generation is bumped — the specific ordering this
    /// task's writer-side fix establishes. Mirrors the #1098 unique-index
    /// pause-seam shape (see `index_manager_unique.rs`'s `PostFlagSetPreGenBumpHook`).
    /// A regression test can park a CREATE here, drive a concurrent tx's
    /// generation-then-flag reads while parked, then resume — proving the
    /// writer's flag-then-gen order is what makes the reader's gen-then-checks
    /// order actually sufficient. NOT `#[cfg(test)]`-gated — cross-crate test
    /// consumer. `None` on every real path.
    pub(in crate::base_index) post_flag_set_pre_gen_bump_hook:
        Arc<arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
}

impl Clone for IndexManager {
    fn clone(&self) -> Self {
        Self {
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            indexes_unique: Arc::clone(&self.indexes_unique),
            write_barrier_flags: self.write_barrier_flags.clone(),
            posting_cache: Arc::clone(&self.posting_cache),
            posting_cache_generation: Arc::clone(&self.posting_cache_generation),
            create_index_backfill_hook: Arc::clone(&self.create_index_backfill_hook),
            drop_index_pause_hook: Arc::clone(&self.drop_index_pause_hook),
            lookup_pause_hook: Arc::clone(&self.lookup_pause_hook),
            lookup_post_scan_pause_hook: Arc::clone(&self.lookup_post_scan_pause_hook),
            generation: Arc::clone(&self.generation),
            dropping_regular: Arc::clone(&self.dropping_regular),
            dropping_unique: Arc::clone(&self.dropping_unique),
            in_flight_builds: Arc::clone(&self.in_flight_builds),
            dirty_sets: Arc::clone(&self.dirty_sets),
            drop_index_post_sweep_hook: Arc::clone(&self.drop_index_post_sweep_hook),
            drop_index_status_pause_hook: Arc::clone(&self.drop_index_status_pause_hook),
            renaming_regular: Arc::clone(&self.renaming_regular),
            renaming_unique: Arc::clone(&self.renaming_unique),
            rename_mid_pause_hook: Arc::clone(&self.rename_mid_pause_hook),
            recover_renames_between_entries_hook: Arc::clone(
                &self.recover_renames_between_entries_hook,
            ),
            reader_gate: self.reader_gate.clone(),
            post_flag_set_pre_gen_bump_hook: Arc::clone(&self.post_flag_set_pre_gen_bump_hook),
        }
    }
}

// ── #1102 test-only pause seam for regular index CREATE ───────────────────
//
// Mirrors the #1098 unique-index pause-seam shape in `index_manager_unique.rs`.
// Fires in all four regular index CREATE paths strictly AFTER the
// `REGULAR_INDEX_EXISTS` flag is set and BEFORE the generation is bumped —
// the specific ordering this task's writer-side fix establishes. A test can
// park a CREATE here, drive a concurrent tx's generation-then-flag reads while
// parked, then resume — proving the writer's flag-then-gen order is what makes
// the reader's gen-then-checks order actually sufficient.
//
// NOT `#[cfg(test)]`-gated — the hook field is public-test-visible for cross-
// crate test consumption (same rationale as `create_index_backfill_hook`).
pub(super) async fn fire_post_flag_set_pre_gen_bump_test_hook(
    hook: &arc_swap::ArcSwapOption<crate::base_index::backfill_pause_hook::BackfillPauseHook>,
) {
    if let Some(hook) = hook.load_full() {
        hook.wait_at_window().await;
    }
}

impl IndexManager {
    /// Создаёт новый менеджер индексов.
    ///
    /// Загружает существующие индексы из служебного хранилища.
    /// Если метаданных нет (таблица новая), создаёт пустые структуры.
    ///
    /// # Аргументы
    ///
    /// * `data_store` — хранилище данных таблицы
    /// * `info_store` — служебное хранилище для индексов
    ///
    /// # Ключи в info_store
    ///
    /// - `system:indexes` — сериализованные метаданные обычных индексов
    /// - `system:indexes_unique` — сериализованные метаданные уникальных индексов
    pub async fn new(
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
    ) -> Result<Self, shamir_storage::error::DbError> {
        // Ключи для хранения метаданных индексов в служебном хранилище
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();

        // Загружаем обычные индексы или создаём пустую структуру
        let indexes = match info_store.get(indexes_key.clone().into()).await {
            Ok(bytes) => {
                // F-72 (#899): `decode_bytes` tries the current shape first,
                // falling back to the pre-`state` legacy shape (lifted to
                // Ready) before giving up — see `index_info.rs`'s module doc.
                //
                // F-83 (#911): a decode failure here is NECESSARILY genuine
                // corruption — `decode_bytes` already handles both the current
                // shape AND the legacy pre-`state` shape internally and
                // returns `Ok` for each. Surfacing the error as a hard failure
                // (rather than the prior `unwrap_or_else(|_| IndexInfo::new())`,
                // which silently discarded the caller's existing index
                // definitions) is the documented promise in `decode_bytes`'s
                // own doc comment. The `NotFound` arm below is the ONLY
                // legitimate "no info yet" case (a brand-new table whose blob
                // was never written). Mirrors `sorted_index_manager::load`'s
                // corruption→`DbError::Codec` propagation.
                IndexInfo::decode_bytes(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:indexes decode failed (genuine corruption — \
                         neither current nor pre-`state` legacy shape): {e}"
                    ))
                })?
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        // Загружаем уникальные индексы или создаём пустую структуру
        let indexes_unique = match info_store.get(indexes_unique_key.clone().into()).await {
            Ok(bytes) => {
                // F-83 (#911): same corruption policy as the `indexes` arm
                // above. For the UNIQUE family a silent `IndexInfo::new()`
                // substitution is worse than data loss — it zeroes
                // `has_indexes_unique_flag`, which flips
                // `WriteBarrierFlags::with_regular_and_unique_index_exists(false, false)`,
                // so every writer skips unique validation and accepts
                // duplicates into a column the schema still treats as
                // unique; the NEXT persist then writes the empty set back to
                // disk, making the loss permanent. Propagate the error.
                IndexInfo::decode_bytes(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:indexes_unique decode failed (genuine corruption — \
                         neither current nor pre-`state` legacy shape): {e}"
                    ))
                })?
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        // Сохраняем флаги наличия индексов до заворачивания в Arc
        let has_indexes_flag = indexes.is_enabled();
        let has_indexes_unique_flag = indexes_unique.is_enabled();

        let manager = Self {
            data_store,
            info_store,
            indexes: Arc::new(indexes),
            indexes_unique: Arc::new(indexes_unique),
            // F-69 (#896): standalone construction gets its own fresh packed
            // word (no `TableManager` exists yet to share it with — callers
            // that DO need the shared word, i.e. `TableManager::create`,
            // fold their own bits into THIS SAME instance via
            // `write_barrier_flags()` right after construction, rather than
            // this manager adopting an externally-supplied `Arc`. This keeps
            // every standalone/test/bench caller of `IndexManager::new`
            // (dozens of call sites across `shamir-index` and
            // `shamir-engine`) working unchanged. #1102: the packed word now
            // also carries `REGULAR_INDEX_EXISTS` (bit 6).
            write_barrier_flags: WriteBarrierFlags::with_regular_and_unique_index_exists(
                has_indexes_flag,
                has_indexes_unique_flag,
            ),
            posting_cache: Arc::new(DashMap::with_capacity_and_hasher(
                POSTING_CACHE_CAP,
                THasher::default(),
            )),
            posting_cache_generation: Arc::new(AtomicU64::new(0)),
            create_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            drop_index_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            lookup_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            lookup_post_scan_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            generation: Arc::new(AtomicU64::new(0)),
            dropping_regular: Arc::new(Mutex::new(BTreeMap::new())),
            dropping_unique: Arc::new(Mutex::new(BTreeMap::new())),
            in_flight_builds: Arc::new(scc::HashMap::with_hasher(THasher::default())),
            dirty_sets: Arc::new(Mutex::new(BTreeMap::new())),
            drop_index_post_sweep_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            drop_index_status_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            renaming_regular: Arc::new(Mutex::new(BTreeMap::new())),
            renaming_unique: Arc::new(Mutex::new(BTreeMap::new())),
            rename_mid_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            recover_renames_between_entries_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            reader_gate: crate::reader_drain_gate::ReaderDrainGate::new(),
            post_flag_set_pre_gen_bump_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
        };

        // P0-3 (#959): resume any in-progress DROP INDEX operations that were
        // interrupted by a crash between the tombstone write and the final
        // IndexInfo persist. The tombstone is persisted under a separate
        // `system:idx_drop` / `system:uidx_drop` key;
        // if present, the recovery path re-runs the (idempotent) sweep and
        // removes the stale definition so the planner never resurrects a
        // fully-broken "Ready but no postings" index. See
        // `recover_in_progress_drops`'s doc for the full crash-state matrix.
        manager.recover_in_progress_drops().await?;

        // Синхронизируем флаги с состоянием IndexInfo
        manager.sync_flags();

        Ok(manager)
    }
}
