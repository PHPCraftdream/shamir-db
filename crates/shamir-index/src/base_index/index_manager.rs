//! Менеджер индексов таблицы.
//!
//! Отвечает за управление индексами конкретной таблицы:
//! - Создание и удаление индексов
//! - Поддержание индексов в актуальном состоянии при операциях CRUD
//! - Персистентное хранение метаданных индексов
//!
//! # Архитектура
//!
//! Индексы делятся на два типа:
//! - Обычные (`indexes`) — позволяют быстро находить записи по значению
//! - Уникальные (`indexes_unique`) — гарантируют уникальность значения
//!
//! Для быстрой проверки наличия индексов используются атомарные флаги,
//! что позволяет избежать блокировок на чтение в большинстве случаев.

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::index_keys::{
    build_index_key, build_index_key_from_record, build_posting_key,
};
use crate::base_index::index_record_key::IndexRecordKey;
use crate::base_index::write_barrier_flags::{WriteBarrierFlags, UNIQUE_INDEX_EXISTS};
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use dashmap::DashMap;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_tx::{IndexFamily, Provenance};
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// R0-B (#1008): [`Provenance`] for an op planned against `def` on the
/// REGULAR (non-unique) base_index family. See
/// `IndexDefinition::provenance`'s doc.
fn regular_provenance(def: &IndexDefinition) -> Provenance {
    def.provenance(IndexFamily::Regular)
}

/// Log CREATE INDEX backfill progress at most this often (avoids spamming the
/// log on every batch of a large table scan).
const BACKFILL_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum number of posting-list entries cached in memory per
/// `IndexManager`. Hit on a cached entry is a single `HashMap::get`
/// + `Arc::clone`; miss falls back to `info_store.get` + bincode
///   deserialise. Capacity is intentionally small — typical workloads
///   (admin UIs, filter-by-status, find-by-id) concentrate on a handful
///   of values per index.
const POSTING_CACHE_CAP: usize = 512;

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
    pub(super) data_store: Arc<dyn Store>,
    /// Служебное хранилище для метаданных и записей индексов
    pub(super) info_store: Arc<dyn Store>,

    /// Метаданные обычных индексов (неуникальных)
    /// IndexInfo использует DashMap внутри, поэтому thread-safe без дополнительной синхронизации
    pub(super) indexes: Arc<IndexInfo>,
    /// Метаданные уникальных индексов
    pub(super) indexes_unique: Arc<IndexInfo>,

    /// Атомарный флаг: есть ли хоть один обычный индекс
    /// Arc позволяет всем клонам видеть одно и то же состояние
    pub(super) has_indexes: Arc<AtomicBool>,
    /// F-69 (#896, P0): the [`UNIQUE_INDEX_EXISTS`] bit of the SAME packed
    /// word `TableManager` (`shamir-engine`) shares as the single atomic
    /// backing `needs_write_barrier()`. Replaces the former standalone
    /// `has_indexes_unique: Arc<AtomicBool>` (which was loaded `Relaxed`,
    /// the exact torn-read bug this task closes — see
    /// `write_barrier_flags.rs`'s module doc for the full writeup). This
    /// manager is the exclusive write authority for this bit; `TableManager`
    /// clones the same `Arc<AtomicU8>` and owns the other five bits.
    pub(super) write_barrier_flags: WriteBarrierFlags,

    /// **Opt G** — in-memory cache for posting lists. Keys are the
    /// raw physical index keys (`build_index_key(...).to_bytes()`);
    /// values are `Arc<[RecordId]>` — a sorted, duplicate-free,
    /// immutable slice (audit 3.2 / task #499). Shared between hits so
    /// the lookup hot path is `HashMap::get` + `Arc::clone` (O(1)); the
    /// consumer then iterates a contiguous, cache-friendly buffer
    /// instead of chasing `BTreeSet` tree-node pointers.
    ///
    /// Invalidated by every write hook on the affected
    /// `(index_name, value)` key. Bounded — when full we evict an
    /// arbitrary entry (exact LRU not worth the dep here; index
    /// hotsets are typically small).
    ///
    /// `DashMap` replaces the previous `Mutex<HashMap>` so concurrent
    /// readers never serialise through a single lock — each shard
    /// is independently lockable and the read path on a cache hit is
    /// fully lock-free against unrelated index keys. Cache hits on
    /// the same shard still take the per-shard read lock.
    pub(super) posting_cache: Arc<DashMap<Bytes, Arc<[RecordId]>, THasher>>,

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
    pub(super) create_index_backfill_hook:
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
    pub(super) drop_index_pause_hook:
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
    pub(super) generation: Arc<AtomicU64>,

    /// P0-3 (#959): in-memory mirror of the persisted tombstone set for
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
    /// `std::sync::Mutex` is the sanctioned low-frequency fallback here
    /// (CLAUDE.md: "only low-frequency/setup fallbacks, justified inline").
    /// DROP INDEX is a DDL operation — contention is nil in normal
    /// operation and the set is empty 99.999 % of the time. A `DashSet`
    /// would be unjustified overkill for a guard that fires once per DDL.
    /// The lock is NEVER held across an `.await` point.
    pub(super) dropping_regular: Arc<Mutex<BTreeSet<u64>>>,
    /// P0-3 (#959): same as `dropping_regular`, for the unique family.
    pub(super) dropping_unique: Arc<Mutex<BTreeSet<u64>>>,

    /// P0-3 (#959): test-only deterministic pause point that fires AFTER
    /// the posting sweep but BEFORE the reduced `IndexInfo` is persisted —
    /// the exact crash window sub-bug 3c tests. A regression test installs
    /// this hook, parks `drop_index` / `drop_unique_index` mid-drop, drops
    /// the manager (simulating a crash), then constructs a fresh
    /// `IndexManager::new` against the SAME `info_store` and asserts the
    /// recovery path finishes the drop instead of resurrecting a broken
    /// `Ready` index. NOT `#[cfg(test)]`-gated — cross-crate test consumer,
    /// same reason as `drop_index_pause_hook`. `None` on every real path.
    pub(super) drop_index_post_sweep_hook:
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
    pub(super) renaming_regular: Arc<Mutex<BTreeMap<u64, HashRenameTombstone>>>,
    /// #997: same as `renaming_regular`, for the UNIQUE (hash,
    /// `is_unique=1`) family. The unique path is the SEVERE crash case
    /// (drop-OLD-first → create-NEW), so the tombstone's stored `paths`
    /// are the ONLY way to rebuild the constraint after a crash.
    pub(super) renaming_unique: Arc<Mutex<BTreeMap<u64, HashRenameTombstone>>>,

    /// #997: test-only deterministic pause point that fires in
    /// `TableManager::rename_index`'s regular+unique paths AFTER the
    /// tombstone is written but BEFORE the second mutating step
    /// (`drop_index` for regular, `create_unique_index_body` for unique) —
    /// the exact mid-rename crash window. A regression test installs this
    /// from `shamir-engine`'s test binary, so (like `drop_index_pause_hook`)
    /// it is NOT `#[cfg(test)]`-gated. `None` on every real path; one
    /// uncontended `ArcSwapOption::load_full()` Acquire load at the call
    /// site. Mirrors #962's `rename_rekey_pause_hook`.
    pub(super) rename_mid_pause_hook:
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
    pub(super) recover_renames_between_entries_hook:
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
    pub(super) reader_gate: crate::reader_drain_gate::ReaderDrainGate,
}

impl Clone for IndexManager {
    fn clone(&self) -> Self {
        Self {
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            indexes_unique: Arc::clone(&self.indexes_unique),
            has_indexes: Arc::clone(&self.has_indexes),
            write_barrier_flags: self.write_barrier_flags.clone(),
            posting_cache: Arc::clone(&self.posting_cache),
            create_index_backfill_hook: Arc::clone(&self.create_index_backfill_hook),
            drop_index_pause_hook: Arc::clone(&self.drop_index_pause_hook),
            generation: Arc::clone(&self.generation),
            dropping_regular: Arc::clone(&self.dropping_regular),
            dropping_unique: Arc::clone(&self.dropping_unique),
            drop_index_post_sweep_hook: Arc::clone(&self.drop_index_post_sweep_hook),
            renaming_regular: Arc::clone(&self.renaming_regular),
            renaming_unique: Arc::clone(&self.renaming_unique),
            rename_mid_pause_hook: Arc::clone(&self.rename_mid_pause_hook),
            recover_renames_between_entries_hook: Arc::clone(
                &self.recover_renames_between_entries_hook,
            ),
            reader_gate: self.reader_gate.clone(),
        }
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
                // `WriteBarrierFlags::with_unique_index_exists(false)`, so
                // every writer skips unique validation and accepts
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
            has_indexes: Arc::new(AtomicBool::new(has_indexes_flag)),
            // F-69 (#896): standalone construction gets its own fresh packed
            // word (no `TableManager` exists yet to share it with — callers
            // that DO need the shared word, i.e. `TableManager::create`,
            // fold their own bits into THIS SAME instance via
            // `write_barrier_flags()` right after construction, rather than
            // this manager adopting an externally-supplied `Arc`. This keeps
            // every standalone/test/bench caller of `IndexManager::new`
            // (dozens of call sites across `shamir-index` and
            // `shamir-engine`) working unchanged.
            write_barrier_flags: WriteBarrierFlags::with_unique_index_exists(
                has_indexes_unique_flag,
            ),
            posting_cache: Arc::new(DashMap::with_capacity_and_hasher(
                POSTING_CACHE_CAP,
                THasher::default(),
            )),
            create_index_backfill_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            drop_index_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            generation: Arc::new(AtomicU64::new(0)),
            dropping_regular: Arc::new(Mutex::new(BTreeSet::new())),
            dropping_unique: Arc::new(Mutex::new(BTreeSet::new())),
            drop_index_post_sweep_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            renaming_regular: Arc::new(Mutex::new(BTreeMap::new())),
            renaming_unique: Arc::new(Mutex::new(BTreeMap::new())),
            rename_mid_pause_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            recover_renames_between_entries_hook: Arc::new(arc_swap::ArcSwapOption::empty()),
            reader_gate: crate::reader_drain_gate::ReaderDrainGate::new(),
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

    /// F-72 (#899, P0) test-only: install (or clear with `None`) the
    /// deterministic `create_index_from_records` backfill pause hook. Not
    /// `#[cfg(test)]`-gated — see the field's doc for why (cross-crate test
    /// consumer).
    pub fn set_create_index_backfill_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.create_index_backfill_hook.store(hook);
    }

    /// F-76 (#903) test-only: install (or clear with `None`) the deterministic
    /// `drop_index` / `drop_unique_index` pause hook (fires between the
    /// definition retirement and the posting sweep). Not `#[cfg(test)]`-gated
    /// — cross-crate test consumer, same reason as
    /// `set_create_index_backfill_hook`. See `drop_index_pause_hook`'s field
    /// doc.
    pub fn set_drop_index_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_pause_hook.store(hook);
    }

    /// P0-3 (#959) test-only: install (or clear with `None`) the deterministic
    /// post-sweep pause hook (fires after the posting sweep, before the
    /// reduced `IndexInfo` is persisted — the exact crash window sub-bug 3c
    /// tests). Not `#[cfg(test)]`-gated — cross-crate test consumer, same
    /// reason as `set_drop_index_pause_hook`.
    pub fn set_drop_index_post_sweep_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_post_sweep_hook.store(hook);
    }

    /// #997 test-only: install (or clear with `None`) the deterministic
    /// rename mid-pause hook (fires after the tombstone is written, before
    /// the second mutating step in `rename_index`). NOT `#[cfg(test)]`-gated
    /// — cross-crate test consumer, same reason as the other hooks.
    pub fn set_rename_mid_pause_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.rename_mid_pause_hook.store(hook);
    }

    /// #997 test-only: fire the rename mid-point pause hook if installed.
    /// Called by `TableManager::rename_index` between the two mutating steps
    /// of each hash-rename path. `None` on every real path — one uncontended
    /// `ArcSwapOption::load_full()` Acquire load.
    pub async fn maybe_pause_rename_mid(&self) {
        if let Some(hook) = self.rename_mid_pause_hook.load_full() {
            hook.wait_at_window().await;
        }
    }

    /// #997 post-review fix (task #1000) test-only: install (or clear with
    /// `None`) the deterministic pause point fired between entries in
    /// `TableManager::recover_hash_renames`'s per-family reconciliation
    /// loop. See the field's doc for the crash window this simulates.
    pub fn set_recover_renames_between_entries_hook(
        &self,
        hook: Option<Arc<crate::base_index::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.recover_renames_between_entries_hook.store(hook);
    }

    /// #997 post-review fix (task #1000) test-only: fire the
    /// between-entries recovery pause hook if installed. Called by
    /// `TableManager::recover_hash_renames` after each tombstone entry is
    /// fully reconciled. `None` on every real path — one uncontended
    /// `ArcSwapOption::load_full()` Acquire load.
    pub async fn maybe_pause_recover_renames_between_entries(&self) {
        if let Some(hook) = self.recover_renames_between_entries_hook.load_full() {
            hook.wait_at_window().await;
        }
    }

    /// Синхронизирует атомарные флаги с реальным состоянием индексов.
    fn sync_flags(&self) {
        self.has_indexes
            .store(self.indexes.is_enabled(), Ordering::Release);
        self.write_barrier_flags
            .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
    }

    // ========================================================================
    // P0-3 (#959) — DROP INDEX durable tombstone + crash recovery
    // ========================================================================
    //
    // Sub-bug 3c: a crash between the posting sweep (step 3 of `drop_index`)
    // and the IndexInfo persist (step 4) resurrects a fully-broken "Ready
    // but no postings" index after restart. The fix: persist a tombstone
    // BEFORE the sweep, and clear it AFTER the persist. On restart,
    // `recover_in_progress_drops` resumes any tombstoned-but-incomplete drop.
    //
    // Sub-bug 3b: a name in the tombstone set is rejected by CREATE INDEX
    // until the tombstone clears, preventing ghost postings from namespace
    // reuse.
    //
    // Tombstone shape: `Vec<u64>` serialized via bincode under
    // `system:idx_drop` (regular) / `system:uidx_drop`
    // (unique). A separate key was chosen over extending `IndexInfo`'s
    // serialized shape to avoid touching `IndexInfo::decode_bytes`'s delicate
    // bincode forward-compat fallback chain (current-shape → pre-`state`
    // legacy shape). An absent key or empty vec means "no in-progress drops".
    //
    // **CRITICAL**: the key names `"idx_drop"` and `"uidx_drop"` are short by
    // design — `RecordId::system(name)` truncates `name` to 12 bytes, so the
    // original candidates `"indexes_dropping"` / `"indexes_unique_dropping"`
    // collided with `"indexes_unique"` (both truncate to `"indexes_uniq"`).

    /// P0-3 (#959): persist the current in-memory `dropping_regular` set to
    /// `info_store` under `system:idx_drop`. Serializes the set as a
    /// `Vec<u64>` (deterministic order via `BTreeSet`). An empty set writes
    /// an empty `Vec<u64>` (NOT a key deletion) so the load path handles
    /// both `NotFound` and empty-vec uniformly.
    pub(super) async fn save_dropping_regular(&self) -> DbResult<()> {
        let snapshot: Vec<u64> = {
            let set = self.dropping_regular.lock().unwrap();
            set.iter().copied().collect()
        };
        let key = RecordId::system("idx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3 (#959): persist the current in-memory `dropping_unique` set.
    /// Mirror of `save_dropping_regular` for the unique family.
    pub(super) async fn save_dropping_unique(&self) -> DbResult<()> {
        let snapshot: Vec<u64> = {
            let set = self.dropping_unique.lock().unwrap();
            set.iter().copied().collect()
        };
        let key = RecordId::system("uidx_drop").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// P0-3 (#959): load a persisted dropping set from `info_store`. Returns
    /// an empty `Vec` if the key is absent (`NotFound`) or contains an empty
    /// vec — both mean "no in-progress drops".
    pub(super) async fn load_dropping_set(&self, is_unique: bool) -> DbResult<Vec<u64>> {
        let key_str = if is_unique { "uidx_drop" } else { "idx_drop" };
        let key = RecordId::system(key_str).to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<u64>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:{key_str} decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// P0-3 (#959): add `name_interned` to the in-memory dropping set, then
    /// persist the updated set durably. MUST be called BEFORE the sweep in
    /// `drop_index` / `drop_unique_index` so a crash at any subsequent point
    /// is recoverable.
    ///
    /// In-memory update happens FIRST (so a concurrent CREATE INDEX
    /// immediately sees the name as guarded), then the persist follows. If
    /// the persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the drop MUST NOT proceed without a durable
    /// tombstone. A crash between the in-memory update and the persist is
    /// safe: nothing has been swept or persisted yet, so the on-disk state
    /// is unchanged (old IndexInfo, postings intact).
    pub(super) async fn add_to_dropping(
        &self,
        is_unique: bool,
        name_interned: u64,
    ) -> DbResult<()> {
        let dropping = if is_unique {
            &self.dropping_unique
        } else {
            &self.dropping_regular
        };
        {
            let mut set = dropping.lock().unwrap();
            set.insert(name_interned);
        }
        let result = if is_unique {
            self.save_dropping_unique().await
        } else {
            self.save_dropping_regular().await
        };
        if result.is_err() {
            // Roll back the in-memory set so the guard stays consistent.
            let mut set = dropping.lock().unwrap();
            set.remove(&name_interned);
        }
        result
    }

    /// P0-3 (#959): clear `name_interned` from the persisted tombstone, then
    /// from the in-memory set. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state: a crash
    /// between persist and in-memory update leaves a stale in-memory entry
    /// that dies with the process, while the on-disk tombstone is already
    /// correct.
    ///
    /// MUST be called AFTER `save_index_info` / `save_index_info_unique`
    /// (the reduced IndexInfo must be durable first). If the process crashes
    /// between the IndexInfo persist and this clear, recovery sees the
    /// tombstone but the def is already gone from IndexInfo — it just clears
    /// the tombstone (a no-op sweep).
    pub(super) async fn clear_from_dropping(
        &self,
        is_unique: bool,
        name_interned: u64,
    ) -> DbResult<()> {
        let dropping = if is_unique {
            &self.dropping_unique
        } else {
            &self.dropping_regular
        };
        // Compute the snapshot without the entry (do NOT modify in-memory yet).
        let snapshot: Vec<u64> = {
            let set = dropping.lock().unwrap();
            set.iter()
                .filter(|&&k| k != name_interned)
                .copied()
                .collect()
        };
        // Persist first.
        let key_str = if is_unique { "uidx_drop" } else { "idx_drop" };
        let key = RecordId::system(key_str).to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        // Only now update in-memory.
        {
            let mut set = dropping.lock().unwrap();
            set.remove(&name_interned);
        }
        Ok(())
    }

    // ========================================================================
    // #997 — RENAME INDEX (regular/unique hash) durable tombstone + crash
    // recovery. Mirrors #959's `idx_drop`/`uidx_drop` (same `IndexManager`
    // owner) and #962's `sidx_ren` (same operation, sorted family).
    // ========================================================================
    //
    // The regular/unique hash RENAME is a drop+rebuild (the hash physical
    // key mixes `name_interned` into h1/h2, so postings cannot be rekeyed —
    // see `rename_index`'s doc). The tombstone therefore carries enough to
    // rebuild from nothing: `HashRenameTombstone { old_name, new_name,
    // paths }`. `TableManager::rename_index` writes the tombstone BEFORE
    // the first mutating step and clears it AFTER the last; the engine-side
    // `TableManager::recover_hash_renames` finishes any interrupted rename
    // on restart (it needs the record stream + interner for a backfill, so
    // it cannot live on `IndexManager`).
    //
    // Tombstone shape: `Vec<HashRenameTombstone>` serialized via bincode
    // under `system:idx_ren` (regular) / `system:uidx_ren` (unique). An
    // absent key or empty vec means "no in-progress renames".
    //
    // **CRITICAL** (the hard-won #959 collision lesson): `RecordId::system(name)`
    // truncates `name` to 12 bytes (see `shamir_types::types::record_id::system`).
    // `"idx_ren"` (7 bytes) is verified collision-free against every other
    // base_index system key:
    //   `"idx_drop"` → [0,0,0,0, 69,64,78,5f,64,72,6f,70, 00,00,00,00]  (8 bytes)
    //   `"idx_ren"`  → [0,0,0,0, 69,64,78,5f,72,65,6e,00, 00,00,00,00]  (7 bytes)
    // They share the first 4 bytes (`idx_`) but diverge at byte 8 (within
    // the name): `0x64` (`d`) vs `0x72` (`r`) — NO collision.
    //   `"indexes"`  → [0,0,0,0, 69,6e,64,65,78,65,73,00, 00,00,00,00]
    //   `"idx_ren"`  → [0,0,0,0, 69,64,78,5f,72,65,6e,00, 00,00,00,00]
    // Diverge at byte 5 (`n`=`0x6e` vs `d`=`0x64`) — NO collision.
    //
    // `"uidx_ren"` (8 bytes) is verified collision-free:
    //   `"uidx_drop"`    → [0,0,0,0, 75,69,64,78,5f,64,72,6f,70,00,00,00]
    //   `"uidx_ren"`     → [0,0,0,0, 75,69,64,78,5f,72,65,6e,00,00,00,00]
    // Diverge at byte 9 (`d`=`0x64` vs `r`=`0x72`) — NO collision.
    //   `"indexes_uniq"` → [0,0,0,0, 69,6e,64,65,78,65,73,5f,75,6e,69,71]  (truncated 12)
    //   `"uidx_ren"`     → [0,0,0,0, 75,69,64,78,5f,72,65,6e,00,00,00,00]
    // Diverge at byte 4 (`i`=`0x69` vs `u`=`0x75`) — NO collision.

    /// #997: persist the in-memory `renaming_regular` map to `info_store`
    /// under `system:idx_ren`. Serializes as a `Vec<HashRenameTombstone>`
    /// (deterministic order via `BTreeMap`). An empty map writes an empty
    /// `Vec` (NOT a key deletion) so the load path handles `NotFound` and
    /// empty-vec uniformly. Mirrors #959's `save_dropping_regular`.
    pub async fn save_renaming_regular(&self) -> DbResult<()> {
        let snapshot: Vec<HashRenameTombstone> = {
            let map = self.renaming_regular.lock().unwrap();
            map.values().cloned().collect()
        };
        let key = RecordId::system("idx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// #997: persist the in-memory `renaming_unique` map. Mirror of
    /// `save_renaming_regular` for the unique family.
    pub async fn save_renaming_unique(&self) -> DbResult<()> {
        let snapshot: Vec<HashRenameTombstone> = {
            let map = self.renaming_unique.lock().unwrap();
            map.values().cloned().collect()
        };
        let key = RecordId::system("uidx_ren").to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        Ok(())
    }

    /// #997: load a persisted rename tombstone list from `info_store`.
    /// Returns an empty `Vec` if the key is absent (`NotFound`) or contains
    /// an empty vec — both mean "no in-progress renames". Mirrors #959's
    /// `load_dropping_set` (adapted for the richer payload type).
    pub async fn load_renaming_list(&self, is_unique: bool) -> DbResult<Vec<HashRenameTombstone>> {
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        match self.info_store.get(key.into()).await {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(Vec::new());
                }
                bincode::deserialize::<Vec<HashRenameTombstone>>(&bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "system:{key_str} decode failed: {e}"
                    ))
                })
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// #997: record a rename in the in-memory map, then persist durably.
    /// MUST be called BEFORE the first mutating step in the rename
    /// (`create_index` for regular, `drop_unique_index` for unique) so a
    /// crash at any subsequent point is recoverable by
    /// `TableManager::recover_hash_renames`.
    ///
    /// In-memory update happens FIRST, then the persist follows. If the
    /// persist fails, the in-memory update is rolled back and the caller
    /// receives `Err` — the rename MUST NOT proceed without a durable
    /// tombstone. Mirrors #959's `add_to_dropping`.
    pub async fn add_to_renaming(
        &self,
        is_unique: bool,
        old_name_interned: u64,
        entry: HashRenameTombstone,
    ) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        {
            let mut map = renaming.lock().unwrap();
            map.insert(old_name_interned, entry);
        }
        let result = if is_unique {
            self.save_renaming_unique().await
        } else {
            self.save_renaming_regular().await
        };
        if result.is_err() {
            let mut map = renaming.lock().unwrap();
            map.remove(&old_name_interned);
        }
        result
    }

    /// #997: clear `old_name_interned` from the persisted tombstone, then
    /// from the in-memory map. Persist-first ordering ensures the on-disk
    /// state is always at least as advanced as the in-memory state.
    ///
    /// MUST be called AFTER the last mutating step succeeds (the new index
    /// is registered + backfilled for regular; the new unique index is
    /// registered + backfilled for unique). If the process crashes between
    /// the last step and this clear, recovery sees the tombstone and
    /// reconciles — see `TableManager::recover_hash_renames`'s crash-state
    /// matrix. Mirrors #959's `clear_from_dropping`.
    pub async fn clear_from_renaming(
        &self,
        is_unique: bool,
        old_name_interned: u64,
    ) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        let snapshot: Vec<HashRenameTombstone> = {
            let map = renaming.lock().unwrap();
            map.iter()
                .filter(|(&k, _)| k != old_name_interned)
                .map(|(_, v)| v.clone())
                .collect()
        };
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        let bytes = bincode::serialize(&snapshot)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(bytes)).await?;
        {
            let mut map = renaming.lock().unwrap();
            map.remove(&old_name_interned);
        }
        Ok(())
    }

    /// #997 (post-review fix): clear the ENTIRE persisted rename tombstone
    /// list for one family by writing an explicit empty `Vec`, bypassing the
    /// in-memory `renaming_regular`/`renaming_unique` map entirely.
    ///
    /// `TableManager::recover_hash_renames` MUST use this once per family
    /// after its whole reconciliation loop finishes, NOT `clear_from_renaming`
    /// per entry. At open time `renaming_regular`/`renaming_unique` start
    /// empty (`IndexManager::new`) and `load_renaming_list` never rehydrates
    /// them — it only returns a local `Vec`. So a per-entry
    /// `clear_from_renaming` call during recovery would derive its snapshot
    /// from that empty map and persist `[]` after the FIRST entry, silently
    /// discarding the durable tombstone for every NOT-YET-recovered entry. If
    /// recovery then failed or the process crashed again before finishing the
    /// remaining entries, their stranded state would become permanently
    /// invisible on the next restart. Mirrors #959's `recover_in_progress_drops`,
    /// which writes an explicit empty `Vec<u64>` once after its loop instead
    /// of calling `clear_from_dropping` per entry.
    pub async fn clear_all_renaming(&self, is_unique: bool) -> DbResult<()> {
        let renaming = if is_unique {
            &self.renaming_unique
        } else {
            &self.renaming_regular
        };
        let key_str = if is_unique { "uidx_ren" } else { "idx_ren" };
        let key = RecordId::system(key_str).to_bytes();
        let empty = bincode::serialize(&Vec::<HashRenameTombstone>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(key.into(), Bytes::from(empty)).await?;
        {
            let mut map = renaming.lock().unwrap();
            map.clear();
        }
        Ok(())
    }

    /// P0-3 (#959): sweep all posting entries for the given index prefix.
    /// Extracted from `drop_index` / `drop_unique_index` so the recovery
    /// path can re-run it idempotently. Removing already-removed keys is a
    /// no-op on every `Store` backend.
    pub(super) async fn sweep_index_postings(
        &self,
        is_unique: bool,
        name_interned: u64,
    ) -> DbResult<()> {
        let prefix = IndexRecordKey::new(is_unique, name_interned).to_prefix_bytes();
        use futures::StreamExt;
        let mut to_remove: Vec<RecordKey> = Vec::new();
        let mut stream = self
            .info_store
            .scan_prefix_stream(prefix.clone(), FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            for (key, _) in batch_result? {
                to_remove.push(key);
            }
        }
        if !to_remove.is_empty() {
            let _ = self.info_store.remove_many(to_remove).await?;
        }
        self.posting_cache.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    /// P0-3 (#959): open-time recovery for DROP INDEX operations interrupted
    /// by a crash. Called from `IndexManager::new` AFTER definitions are
    /// loaded but BEFORE the manager is returned to the caller.
    ///
    /// # Crash-state matrix
    ///
    /// | crash point                    | IndexInfo has def? | tombstone? | recovery action            |
    /// |--------------------------------|--------------------|------------|----------------------------|
    /// | after tombstone-write, pre-sweep | yes (Ready)        | yes        | remove def, sweep, persist |
    /// | after sweep, pre-persist        | yes (Ready)        | yes        | remove def, sweep (no-op), persist |
    /// | after persist, pre-clear        | no                 | yes        | sweep (no-op), clear tombstone |
    ///
    /// In every case the recovery leaves the manager in a consistent state:
    /// the def is gone from IndexInfo, its postings are swept, and the
    /// tombstone is cleared. The sweep is idempotent (removing already-removed
    /// keys is a no-op), so calling recovery twice (two restart attempts) is
    /// a clean no-op on the second call.
    pub(super) async fn recover_in_progress_drops(&self) -> DbResult<()> {
        let dropping_regular = self.load_dropping_set(false).await?;
        let dropping_unique = self.load_dropping_set(true).await?;

        if dropping_regular.is_empty() && dropping_unique.is_empty() {
            return Ok(());
        }

        log::info!(
            "P0-3 (#959): recovering {} regular + {} unique in-progress DROP(s)",
            dropping_regular.len(),
            dropping_unique.len()
        );

        // ── Regular family ───────────────────────────────────────────────
        let mut regular_changed = false;
        for &name_interned in &dropping_regular {
            if self.indexes.contains(name_interned) {
                // Crash between tombstone-write and IndexInfo-persist:
                // def is still in IndexInfo at Ready. Resume the drop.
                self.indexes.remove_index(name_interned);
                self.bump_generation();
                regular_changed = true;
            }
            // Always run the sweep (idempotent). Covers both the
            // "sweep never ran" and "sweep ran but persist failed" cases.
            self.sweep_index_postings(false, name_interned).await?;
        }
        if regular_changed {
            self.has_indexes
                .store(self.indexes.is_enabled(), Ordering::Release);
            self.save_index_info().await?;
        }

        // ── Unique family ────────────────────────────────────────────────
        let mut unique_changed = false;
        for &name_interned in &dropping_unique {
            if self.indexes_unique.contains(name_interned) {
                self.indexes_unique.remove_index(name_interned);
                self.bump_generation();
                unique_changed = true;
            }
            self.sweep_index_postings(true, name_interned).await?;
        }
        if unique_changed {
            self.write_barrier_flags
                .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
            self.save_index_info_unique().await?;
        }

        // ── Clear both tombstones ────────────────────────────────────────
        // Write empty Vec<u64> for both keys (cheaper than a remove, and
        // the load path treats empty-vec and NotFound identically).
        let empty = bincode::serialize(&Vec::<u64>::new())
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let key_regular = RecordId::system("idx_drop").to_bytes();
        self.info_store
            .set(key_regular.into(), Bytes::from(empty.clone()))
            .await?;
        let key_unique = RecordId::system("uidx_drop").to_bytes();
        self.info_store
            .set(key_unique.into(), Bytes::from(empty))
            .await?;

        // Sync in-memory flags one more time after recovery.
        self.sync_flags();

        log::info!(
            "P0-3 (#959): recovery complete — {} regular + {} unique DROP(s) finalized",
            dropping_regular.len(),
            dropping_unique.len()
        );

        Ok(())
    }

    /// Проверяет, есть ли хоть один обычный индекс.
    ///
    /// Использует атомарное чтение, поэтому очень быстро.
    /// Не требует захвата блокировки.
    pub fn has_indexes(&self) -> bool {
        self.has_indexes.load(Ordering::Relaxed)
    }

    /// Проверяет, есть ли хоть один уникальный индекс.
    ///
    /// F-69 (#896): reads the shared [`WriteBarrierFlags`] word (`SeqCst`) —
    /// previously a standalone `Relaxed` `AtomicBool` load, which is the
    /// exact ordering gap that let a duplicate slip past a unique
    /// constraint racing a concurrent index-create (see
    /// `write_barrier_flags.rs`'s module doc). `SeqCst` here costs nothing
    /// extra in practice (x86 loads are already total-order-respecting;
    /// the fence cost of `SeqCst` is paid on the RMW/store side, not this
    /// load) and is required to keep this bit inside the SAME total order
    /// as `TableManager::needs_write_barrier()`'s single load of this word.
    pub fn has_unique_indexes(&self) -> bool {
        self.write_barrier_flags.is_set(UNIQUE_INDEX_EXISTS)
    }

    /// P0-2 (#958): current base_index `IndexManager` generation (regular +
    /// unique combined). Bumped (monotonic) whenever the set of definitions
    /// changes. The zero-overhead gate value for commit-time base_index
    /// ops-plan re-derivation: a tx captures this at stage time and, at
    /// commit, skips re-derivation entirely unless it has advanced.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// P0-2 (#958): advance the generation counter after a definition
    /// change (create or drop of a regular or unique index). Called by
    /// every create / drop path in this manager and in
    /// `index_manager_unique.rs` (same `impl IndexManager` block).
    /// Mirrors `SortedIndexManager`'s `generation.fetch_add` and
    /// `IndexRegistry`'s equivalent.
    pub(super) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// F-69 (#896): expose the shared packed write-barrier word so
    /// `TableManager` (`shamir-engine`) can fold its own five DDL-intent
    /// bits into the SAME `Arc<AtomicU8>` this manager uses for
    /// [`UNIQUE_INDEX_EXISTS`] — the shape that makes
    /// `needs_write_barrier()` a single atomic load across both crates'
    /// halves of the predicate. See `write_barrier_flags.rs`'s module doc
    /// for the full ownership rationale.
    pub fn write_barrier_flags(&self) -> WriteBarrierFlags {
        self.write_barrier_flags.clone()
    }

    /// Создаёт новый индекс для таблицы.
    ///
    /// Процесс создания:
    /// 1. Регистрирует определение индекса (чтобы live write-hook
    ///    начал поддерживать постинги для конкурентных записей).
    /// 2. Потоково читает `data_store` батчами по `FULL_SCAN_BATCH`
    ///    записей и для каждого батча строит постинги и флашит их
    ///    через `set_many` — память ограничена O(батч), а не O(таблица).
    ///
    /// # Аргументы
    ///
    /// * `index_def` — определение индекса (имя, пути полей, уникальность)
    ///
    /// # Производительность
    ///
    /// Потоковая обработка: каждый батч (`FULL_SCAN_BATCH` записей)
    /// обрабатывается независимо — декодируется, индексируется и
    /// флашится в `info_store` через `set_many`. Пиковая память
    /// ограничена размером батча, а не размером всей таблицы.
    pub async fn create_index(&self, index_def: IndexDefinition) -> DbResult<()> {
        use futures::StreamExt;

        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE INDEX for a name whose DROP
        // is still in flight — a tombstoned name has had (or is having) its
        // postings swept; a fresh create would inherit orphan ghost postings
        // keyed by the same `name_interned`.
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST ──────────────────────────
        // (Same concurrency rationale as `create_index_from_records`.)
        self.indexes.add_index(index_def);
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        // ── Phase 2: incremental backfill, batch-by-batch ───────────────────
        // Each batch is decoded, indexed, and flushed independently. Memory
        // is bounded by the batch size (O(batch)), not the table size.
        // Idempotent: a record also written live by the hook right at the
        // registration boundary yields the same (key, empty-value) pair.
        //
        // NOTE: we decode each record into a full `InnerValue` (same as the
        // materialized path in `create_index_from_records`) to guarantee
        // byte-identical posting keys. The zero-copy `RecordView` lens
        // produces DIFFERENT scalar hashes for some value types (e.g. f64
        // encoding edge cases in the msgpack wire form), so it cannot be
        // used here without breaking index-identity. A follow-up should
        // reconcile the lens's scalar decoding with `InnerValue`'s and
        // then switch this path to `RecordView` for zero-copy indexing.
        //
        // F-2 (#1028): a malformed key (not exactly 16 bytes) or an
        // undecodable value now ABORTS the backfill with a typed
        // `DbError::Codec`, instead of silently `continue`-ing past the
        // row. Fail-open here previously left the row un-indexed while the
        // index was still marked `Ready` — a later query planned through
        // this index would silently never return that row, even though a
        // full scan would find it (worse than the unique-index case #1023
        // already fixed: that one risks an unconstrained duplicate; this
        // one silently drops rows from query results). Mirrors #1023's
        // fail-closed fix for `create_unique_index`'s backfill
        // (`index_manager_unique.rs`).
        let mut count = 0usize;
        let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let mut posting_writes: Vec<(Bytes, Bytes)> =
                Vec::with_capacity(batch.len().min(131_072));
            let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(posting_writes.capacity());
            for (key_bytes, value_bytes) in &batch {
                let arr: [u8; 16] = key_bytes.as_ref().try_into().map_err(|_| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_index backfill: malformed record key, expected 16-byte \
                         RecordId, got {} bytes (genuine corruption — fail-closed, aborting \
                         backfill rather than silently skipping the row)",
                        key_bytes.as_ref().len()
                    ))
                })?;
                let record_id = RecordId(arr);
                let value = InnerValue::from_bytes(value_bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_index backfill: record {record_id:?} value failed to decode: \
                         {e} (fail-closed, aborting backfill rather than silently skipping \
                         the row)"
                    ))
                })?;
                if let Some(irk) = build_index_key_from_record(false, name_interned, &value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, &record_id);
                    posting_writes.push((posting_key, Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }
            if !posting_writes.is_empty() {
                // Boundary: posting keys are built as `Bytes`; the store
                // `set_many` now takes `RecordKey` (byte-identical conversion).
                let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v))
                    .collect();
                self.info_store.set_many(posting_writes).await?;
            }
            for ik in cache_index_keys {
                self.posting_cache.remove(&ik);
            }
        }

        log::info!(
            "Created index '{}' with {} entries (streamed)",
            name_interned,
            count
        );
        Ok(())
    }

    /// FINAL-A: create index and backfill from an already-decoded record
    /// stream instead of `data_store.iter_stream`. Used by `TableManager`
    /// when an MvccStore is attached — the seam (`list_stream`) is the
    /// sole source of truth after FINAL-A.
    ///
    /// # Concurrency (audit A9 — register-first, backfill-second)
    ///
    /// The index definition is registered (`add_index`) BEFORE backfilling
    /// postings. This closes the lost-write window: any concurrent writer
    /// that lands AFTER the snapshot was taken but BEFORE registration is
    /// now caught by the live write-hook (`on_record_created`), which sees
    /// the freshly-registered definition and maintains a posting for it.
    ///
    /// A record captured by BOTH the snapshot AND a concurrent live write
    /// (a narrow overlap right at the registration boundary) produces the
    /// SAME physical posting key — `build_posting_key(index_key, record_id)`
    /// is a pure function of `(is_unique, name_interned, h1, h2, record_id)`
    /// with an EMPTY value (`Bytes::new()`) — so writing it twice is an
    /// idempotent no-op, not a corruption.
    ///
    /// # Planner invisibility + persist ordering (F-72, #899, P0)
    ///
    /// `index_def` is registered at `state = Building` (set by the caller,
    /// `TableManager::create_index` — see that call site) — register-first
    /// is UNCHANGED (it is what closes the lost-write race above); what
    /// changes is that a `Building` definition is invisible to every
    /// PLANNER lookup (`TableManager::find_single_field_index`,
    /// `try_plan_and_index_scan` — both now consult
    /// `IndexManager::iter_indexes_ready`, not the raw `iter_indexes`), so a
    /// concurrent Eq/In/And query cannot be routed to this half-populated
    /// index while Phase 2 below is still streaming postings.
    ///
    /// The FIRST `save_index_info` (right after `add_index`) durably
    /// publishes the `Building` marker BEFORE the backfill starts — so a
    /// crash mid-backfill leaves a durable, planner-invisible `Building`
    /// definition on disk, not a `Ready` one with missing postings. Once the
    /// backfill (Phase 2) completes, the definition is flipped to `Ready`
    /// in-memory and a SECOND `save_index_info` durably persists that flip —
    /// this fixes the pre-F-72 publish-then-persist inversion (the state
    /// flip to `Ready`, i.e. "queryable", now happens no earlier than the
    /// persist that makes it durable, never after — a persist failure at
    /// the SECOND save cannot leave a `Ready`, queryable, durably-unsaved-as-
    /// Ready index behind: `?` propagates the error and the in-memory
    /// `indexes` map still holds the flipped-to-Ready struct, but the
    /// CALLER receives `Err`, so the DDL statement is reported as failed —
    /// see `create_index`'s caller contract). On a genuine backfill error
    /// (Phase 2's `?` — none of today's in-loop steps are fallible in this
    /// specific method, but `plan`/`apply` sibling call sites are — or a
    /// future fallible step), the definition simply never reaches the
    /// second `save_index_info`/flip and stays durably `Building`.
    ///
    /// Unlike index2, the base_index `IndexManager` family has NO automatic
    /// restart-from-scratch self-heal for a `Building` definition at
    /// table-open time (grep-verified: `IndexManager::new` only loads
    /// definitions via `IndexInfo::decode_bytes`, it does not re-run any
    /// backfill). A `Building` definition left behind by a crash/error
    /// therefore stays durably `Building` — permanently planner-invisible,
    /// never silently resurrected as `Ready` — until an operator runs
    /// `TableManager::repair()` (`doctor.rs`), which rebuilds every
    /// definition unconditionally regardless of state. This is an accepted,
    /// explicitly-documented gap: automatic base_index-family self-healing is
    /// out of scope for this task (mirrors the equivalent note on
    /// `create_sorted_index_with_include`).
    pub async fn create_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight (see `create_index`'s matching guard).
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST, at Building ────────────
        // Once registered, the live write-hook starts maintaining postings
        // for every concurrent/new write against this index immediately —
        // but the definition stays planner-invisible until Phase 3 flips it
        // to Ready. `save_index_info` durably persists the `Building` marker
        // so a crash between registration and backfill leaves a correctly-
        // registered (but possibly under-populated), still-planner-invisible
        // index that the doctor `repair()` pass can top up on request.
        self.indexes.add_index(index_def);
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        // ── Phase 2: backfill postings from the snapshot ────────────────────
        // Idempotent: a record also written live by the hook right at the
        // registration boundary yields the same (key, empty-value) pair.
        let mut count = 0usize;
        let mut posting_writes: Vec<(Bytes, Bytes)> = Vec::with_capacity(131072);
        let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(131072);
        for (record_id, value) in &records {
            if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths) {
                let index_key = irk.to_bytes();
                let posting_key = build_posting_key(&index_key, record_id);
                posting_writes.push((posting_key, Bytes::new()));
                cache_index_keys.push(index_key);
                count += 1;
            }
        }
        if !posting_writes.is_empty() {
            // Boundary: posting keys are built as `Bytes`; the store
            // `set_many` now takes `RecordKey` (byte-identical conversion).
            let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                .into_iter()
                .map(|(k, v)| (k.into(), v))
                .collect();
            // P1-2 (#967): if this backfill write fails AFTER Phase 1's
            // `save_index_info` already persisted the Building definition,
            // the index is durably registered but NOT queryable. Enrich
            // the error so the caller knows what happened and how to resolve.
            self.info_store
                .set_many(posting_writes)
                .await
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "CREATE INDEX '{name_interned}': the index definition was \
                     durably registered as Building (Phase 1 persist succeeded), \
                     but the backfill posting write (Phase 2) failed: {e}. The \
                     index is NOT queryable — it remains permanently Building \
                     (planner-invisible) until rebuilt. Call TableManager::verify() \
                     to confirm state, or TableManager::repair() to rebuild it."
                    ))
                })?;
        }
        for ik in cache_index_keys {
            self.posting_cache.remove(&ik);
        }

        // F-72 (#899, P0) test seam: park here (postings written, definition
        // still `Building` and hence planner-invisible until Phase 3 below)
        // if a test installed a pause hook. `None` on every real path — one
        // uncontended `ArcSwapOption::load_full()` Acquire load, no lock, no
        // allocation. NOT `#[cfg(test)]`-gated — see the field's doc for why
        // (a cross-crate test consumer needs this reachable in a normal,
        // non-test compile of this crate). Lets a regression test drive a
        // concurrent READ into the exact window this task closes.
        if let Some(hook) = self.create_index_backfill_hook.load_full() {
            hook.wait_at_window().await;
        }

        // ── Phase 3: flip Building → Ready, persist BEFORE returning ────────
        // F-72 (#899, P0): the state flip and its durable persist are the
        // ONLY point a concurrent planner read may start observing this
        // index. Fixes the publish-then-persist inversion: the in-memory
        // flip happens here, immediately followed by the persist that makes
        // it durable — never the other way around (persist-after-serving is
        // what the OLD single-save-before-backfill shape effectively did
        // for `Ready`-by-default definitions). A failure on THIS save
        // surfaces as `Err` to the DDL caller (the statement is reported
        // failed) while the definition remains `Ready` in THIS process's
        // memory but durably `Building` on disk — an intentional choice
        // (see the method doc): we do not roll back the in-memory flip,
        // because the postings themselves are already fully written and
        // correct; a restart would simply re-observe `Building` from disk
        // and require an operator `repair()` to reconcile, at worst costing
        // a redundant full rebuild, never a correctness gap (the disk state
        // never claims Ready without the postings backing it, and the
        // planner in THIS still-live process is the only place `Ready` is
        // visible without yet being durable — acceptable because the
        // postings backing that Ready view already exist).
        if let Some(def) = self.indexes.get_index(name_interned) {
            let mut ready_def = def;
            ready_def.state = crate::state::IndexState::Ready;
            self.indexes.add_index(ready_def);
        }
        // P1-2 (#967): if this Phase 3 persist fails, the index is Ready
        // in THIS process's memory but durably Building on disk — enrich
        // the error so the caller knows the state split and how to resolve.
        self.save_index_info().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE INDEX '{name_interned}': the backfill completed and the \
                 index was flipped to Ready in memory, but the final durable \
                 persist of the Ready state (Phase 3) failed: {e}. The index is \
                 queryable in THIS process but durably Building on disk — a \
                 restart will reload it as Building (planner-invisible). Call \
                 TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            ))
        })?;

        log::info!(
            "Created index '{}' with {} entries (from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// F-78 (#905): streaming counterpart of [`create_index_from_records`].
    ///
    /// Same externally-observable result and the SAME F-72 Phase 1 → Phase 2
    /// → Phase 3 lifecycle as [`create_index_from_records`]; only Phase 2's
    /// *body* differs — it consumes a `Stream` of decoded record *batches*
    /// instead of one pre-materialised `Vec`, so peak memory is O(batch) not
    /// O(table). Each batch is decoded, indexed, and flushed to `info_store`
    /// via its own `set_many` (one transactional commit per batch on
    /// transactional backends), then dropped before the next batch is read.
    ///
    /// # Why a separate method (not a signature change on the old one)
    ///
    /// [`create_index_from_records`] is preserved byte-for-byte unchanged so
    /// the F-78 correctness-equivalence test can build the SAME index the OLD
    /// (materialise-then-one-`set_many`) way and the NEW (stream-and-batch)
    /// way against identical fixtures and assert the resulting posting SETS
    /// are identical. The streaming rewrite must change HOW the postings are
    /// written, never WHAT gets written.
    ///
    /// # F-72 state machine — UNCHANGED
    ///
    /// Phase 1 (register at `Building`, durable persist) and Phase 3 (flip to
    /// `Ready`, durable persist) are identical to [`create_index_from_records`]
    /// — only Phase 2's materialise-then-iterate body is replaced by a
    /// stream-and-batch body. The `create_index_backfill_hook` pause point
    /// stays exactly where it was (post-Phase-2, pre-Phase-3), so F-72's
    /// regression tests are unaffected.
    ///
    /// # Concurrency — write-delta catch-up is FREE (reconfirmed)
    ///
    /// The caller (`TableManager::create_index`) holds F-70's write barrier
    /// (`begin_write_barrier(REGULAR_INDEX_CREATE)` → raise bit → drain →
    /// `unique_write_lock`) across the ENTIRE Phase 1→2→3 sequence, so no
    /// concurrent writer can land a row *during* this loop — writers observe
    /// `needs_write_barrier() == true` and queue on `unique_write_lock` until
    /// the barrier drops. A row written at the registration boundary (after
    /// the snapshot but before Phase 1's `add_index`) is caught by the LIVE
    /// write-hook that Phase 1 activates — the SAME register-first ordering
    /// `create_index_from_records` already relied on, which this method does
    /// NOT change. So streaming introduces no new lost-write window and needs
    /// no new catch-up mechanism: the existing register-first + live-hook +
    /// barrier ordering already covers it. (The barrier holds writers for the
    /// whole build in BOTH the old and new shapes, so streaming does not by
    /// itself reduce writer queueing — its benefit is peak *memory*, tracked
    /// by the F-78 bench. Reducing writer-blocked time would require releasing
    /// the barrier between batches, which is explicitly out of scope — see the
    /// brief's "do not disturb F-72 / only Phase 2's body is in scope" rule.)
    ///
    /// # Cancel-safety / partial-build residual (unchanged class)
    ///
    /// An `Err` from the stream or a `set_many` (`?` below) leaves the
    /// definition registered but `Building` — it never reaches Phase 3's
    /// flip, so it stays permanently planner-invisible until an operator
    /// `repair()` — the SAME accepted residual as
    /// [`create_index_from_records`] and `create_sorted_index_with_include`.
    pub async fn create_index_from_stream<S>(
        &self,
        index_def: IndexDefinition,
        stream: S,
    ) -> DbResult<()>
    where
        S: futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + Unpin,
    {
        use futures::StreamExt;

        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight (see `create_index`'s matching guard).
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST, at Building ────────────
        // (Identical to `create_index_from_records` — see that method's doc.)
        self.indexes.add_index(index_def);
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        // ── Phase 2: stream + batch-write postings (F-78 body) ──────────────
        // O(batch) peak memory: each batch's posting_writes / cache_index_keys
        // are built, flushed via `set_many`, and dropped before the next batch.
        // Idempotent against a record also written by the live hook at the
        // registration boundary — same (posting_key, empty-value) pair.
        let mut count = 0usize;
        let mut stream = stream;
        // P1-4 (#969): periodic progress log so an operator watching logs can
        // see the DDL is progressing, not hung, during a long backfill scan.
        // Time-gated (not batch-gated) so the cadence is consistent regardless
        // of the caller's batch size.
        let backfill_start = Instant::now();
        let mut last_progress_log = Instant::now();
        let mut batch_no = 0u64;
        // P1-2 (#967): enricher for any Phase 2 backfill failure — the
        // Building definition is already durably persisted by Phase 1 above.
        let enrich_backfill = |e: shamir_storage::error::DbError| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE INDEX '{name_interned}': the index definition was \
                 durably registered as Building (Phase 1 persist succeeded), \
                 but the streaming backfill (Phase 2) failed: {e}. The index \
                 is NOT queryable — it remains permanently Building \
                 (planner-invisible) until rebuilt. Call TableManager::verify() \
                 to confirm state, or TableManager::repair() to rebuild it."
            ))
        };
        while let Some(batch_result) = stream.next().await {
            let batch = match batch_result {
                Ok(b) => b,
                Err(e) => return Err(enrich_backfill(e)),
            };
            let mut posting_writes: Vec<(Bytes, Bytes)> =
                Vec::with_capacity(batch.len().min(131_072));
            let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(posting_writes.capacity());
            for (record_id, value) in &batch {
                if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    posting_writes.push((posting_key, Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }
            if !posting_writes.is_empty() {
                // Boundary: posting keys are built as `Bytes`; the store
                // `set_many` takes `RecordKey` (byte-identical conversion).
                let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v))
                    .collect();
                self.info_store
                    .set_many(posting_writes)
                    .await
                    .map_err(enrich_backfill)?;
            }
            for ik in cache_index_keys {
                self.posting_cache.remove(&ik);
            }
            batch_no += 1;
            if last_progress_log.elapsed() >= BACKFILL_PROGRESS_LOG_INTERVAL {
                log::info!(
                    "CREATE INDEX '{}': backfill in progress — {} rows indexed \
                     across {} batches ({:.1}s elapsed)",
                    name_interned,
                    count,
                    batch_no,
                    backfill_start.elapsed().as_secs_f64()
                );
                last_progress_log = Instant::now();
            }
        }

        // F-72 (#899, P0) test seam — IDENTICAL placement to
        // `create_index_from_records` (post-Phase-2, pre-Phase-3). NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer; see the field's
        // doc. `None` on every real path.
        if let Some(hook) = self.create_index_backfill_hook.load_full() {
            hook.wait_at_window().await;
        }

        // ── Phase 3: flip Building → Ready, persist BEFORE returning ────────
        // (Identical to `create_index_from_records` — see that method's doc.)
        if let Some(def) = self.indexes.get_index(name_interned) {
            let mut ready_def = def;
            ready_def.state = crate::state::IndexState::Ready;
            self.indexes.add_index(ready_def);
        }
        // P1-2 (#967): Phase 3 persist — same enrichment as
        // `create_index_from_records` (see that method's matching comment).
        self.save_index_info().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE INDEX '{name_interned}': the backfill completed and the \
                 index was flipped to Ready in memory, but the final durable \
                 persist of the Ready state (Phase 3) failed: {e}. The index is \
                 queryable in THIS process but durably Building on disk — a \
                 restart will reload it as Building (planner-invisible). Call \
                 TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            ))
        })?;

        log::info!(
            "Created index '{}' with {} entries in {:.1}s (streamed from seam)",
            name_interned,
            count,
            backfill_start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// Удаляет индекс по его имени.
    ///
    /// Процесс удаления (F-76 / #903 — definition retired BEFORE the posting
    /// sweep, the mirror image of F-72's CREATE gate; P0-3 / #959 — durable
    /// tombstone closes the crash-resurrection gap):
    /// 1. Проверяет существование индекса
    /// 2. **P0-3**: persist a durable tombstone (`system:idx_drop`)
    ///    recording that `name_interned` is being dropped — MUST succeed
    ///    before the sweep, so a crash at any later point is recoverable
    /// 3. Retires the definition from the in-memory Vec (planner-invisible
    ///    via RCU swap)
    /// 4. Sweeps all posting entries from `info_store`
    /// 5. Persists the reduced `IndexInfo` (def removed)
    /// 6. **P0-3**: clears the tombstone
    ///
    /// F-76 (#903): the OLD order was sweep → `remove_index`, which left a
    /// window in which a concurrent reader could still select the index via
    /// `iter_indexes_ready` (it was still `Ready` in the Vec) while its
    /// postings were mid-sweep — silently wrong/incomplete results, the
    /// mirror image of the F-72 CREATE bug. The RCU `remove_index` swap
    /// publishes a Vec without this definition atomically; every NEW reader
    /// after this point no longer sees the index and falls back to a full
    /// scan. A reader that already snapshotted the old Vec keeps working
    /// against its own consistent view.
    ///
    /// P0-3 (#959) sub-bug 3c — crash-resurrection: the OLD code had a gap
    /// between the sweep (step 4) and the IndexInfo persist (step 5). If the
    /// process crashed in that gap, the on-disk `IndexInfo` still listed the
    /// index as `Ready`, but all its postings were gone — `IndexManager::new`
    /// would load the stale `Ready` definition and the planner would route
    /// queries to an index with zero postings, silently returning wrong
    /// (empty / missing) results with no error or crash signal. The durable
    /// tombstone (step 2) closes this: on restart, `recover_in_progress_drops`
    /// sees the tombstone, resumes the sweep (idempotent), removes the stale
    /// definition, persists, and clears the tombstone.
    ///
    /// P0-3 (#959) sub-bug 3a — in-flight reader consistency (KNOWN GAP):
    /// A reader that resolved the definition BEFORE `drop_index` retired it
    /// holds an `Arc`-snapshot of the Vec and will continue to route a
    /// lookup through `info_store`'s shared keyspace DURING the sweep. Such
    /// a reader CAN observe a PARTIALLY-swept keyspace — some posting keys
    /// already removed, others not yet — returning an incomplete (but never
    /// wrong/corrupted) result set. `remove_many` only removes; it never
    /// corrupts existing postings. Fully closing this race requires either
    /// (a) a per-backend reader-count epoch mechanism (this codebase does
    /// not have one) or (b) a grace period before the physical sweep — both
    /// are substantial, separately-scoped work. The engine-side mitigation
    /// (`TableManager::drop_index` wrapping the call in
    /// `begin_write_barrier(REGULAR_INDEX_CREATE)`) serializes DROP against
    /// concurrent WRITERS but does NOT serialize against concurrent READERS.
    /// See the module doc on `dropping_regular` for the full write-up.
    ///
    /// # Возвращает
    ///
    /// `true` — индекс существовал и был удалён
    /// `false` — индекс не найден
    pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
        // Быстрая проверка существования индекса
        if !self.indexes.contains(name_interned) {
            return Ok(false);
        }

        // P0-3 (#959): write a durable tombstone BEFORE retiring the
        // definition or sweeping postings. If the process crashes after
        // the sweep but before the reduced IndexInfo is persisted, the
        // on-disk metadata still lists the index as `Ready` — but the
        // tombstone tells `recover_in_progress_drops` to finish the drop
        // rather than resurrecting a broken "Ready but no postings" index.
        // MUST succeed before proceeding; `add_to_dropping` rolls back the
        // in-memory set on persist failure.
        self.add_to_dropping(false, name_interned).await?;

        // F-76 (#903): retire the definition from the planner-visible Vec
        // FIRST (see the method doc). The RCU swap publishes a Vec without
        // this definition atomically.
        let was_removed = self.indexes.remove_index(name_interned);
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.has_indexes
            .store(self.indexes.is_enabled(), Ordering::Release);

        // F-76 test seam: park here (definition already retired, postings not
        // yet swept) if a test installed a pause hook. With the fix, a
        // concurrent read issued while parked here must fall back to a full
        // scan. NOT `#[cfg(test)]`-gated — see `drop_index_pause_hook`'s
        // field doc (cross-crate test consumer).
        if let Some(hook) = self.drop_index_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Sweep the (now orphan, planner-invisible) posting entries.
        // P0-3 (#959): extracted to `sweep_index_postings` so the recovery
        // path can re-run it idempotently.
        // P1-2 (#967): a durable tombstone is already persisted — if this
        // sweep fails, enrich the error with the partial-state context.
        self.sweep_index_postings(false, name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name_interned}': a durable drop tombstone was \
                 persisted and the definition was retired from the planner, but \
                 the posting sweep failed: {e}. On restart, recovery will resume \
                 the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                ))
            })?;

        // P0-3 (#959) test seam — park here (sweep complete, reduced
        // IndexInfo NOT yet persisted) if a test installed the post-sweep
        // hook. This is the exact crash window sub-bug 3c exercises: a
        // "crash" here (dropping the manager) leaves the tombstone on disk
        // but the old IndexInfo, and the recovery path in `new()` must
        // finish the drop. NOT `#[cfg(test)]`-gated — cross-crate test
        // consumer, same reason as `drop_index_pause_hook`.
        if let Some(hook) = self.drop_index_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced IndexInfo (definition removed).
        // P1-2 (#967): the tombstone is still in place — if this persist
        // fails, recovery will see the tombstone and finish the drop.
        if was_removed {
            self.save_index_info().await.map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name_interned}': a durable drop tombstone was \
                     persisted, the definition was retired, and the posting sweep \
                     completed, but persisting the reduced index metadata failed: \
                     {e}. On restart, recovery will finish the drop idempotently. \
                     Call TableManager::verify() to confirm state."
                ))
            })?;
        }

        // P0-3 (#959): clear the tombstone AFTER the reduced IndexInfo is
        // durably persisted. `clear_from_dropping` persists first, then
        // updates in-memory — if the process crashes between persist and
        // in-memory update, the on-disk tombstone is already cleared and
        // the stale in-memory entry dies with the process. If the process
        // crashes BEFORE this call (between IndexInfo persist and tombstone
        // clear), recovery sees the tombstone but the def is already gone
        // from IndexInfo — it just clears the tombstone (a no-op sweep).
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        self.clear_from_dropping(false, name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP INDEX '{name_interned}': the drop is essentially complete \
                 (tombstone persisted, definition retired, sweep done, reduced \
                 metadata persisted), but clearing the drop tombstone failed: {e}. \
                 On restart, recovery will clear the tombstone as a no-op. Call \
                 TableManager::verify() to confirm state."
                ))
            })?;

        Ok(was_removed)
    }

    /// Сохраняет метаданные индексов в служебное хранилище.
    ///
    /// Сериализует IndexInfo через bincode и сохраняет под системным ключом.
    /// Сериализует напрямую без клонирования — IndexInfo::serialize конвертирует
    /// DashMap в BTreeMap внутри себя.
    pub(super) async fn save_index_info(&self) -> DbResult<()> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let bytes = bincode::serialize(&*self.indexes)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store
            .set(indexes_key.into(), Bytes::from(bytes))
            .await?;
        Ok(())
    }

    /// Обработчик события создания записи.
    ///
    /// Добавляет новую запись во все активные индексы.
    /// Вызывается после успешной вставки записи в таблицу.
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор новой записи
    /// * `value` — значение записи
    pub async fn on_record_created(
        &self,
        record_id: &RecordId,
        value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self.plan_record_created(record_id, value).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_created` — returns
    /// `Vec<IndexWriteOp>` instead of writing directly to `info_store`.
    pub async fn plan_record_created(
        &self,
        record_id: &RecordId,
        value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(4);
        for def in self.indexes.iter() {
            if let Some(irk) =
                build_index_key_from_record(false, def.name_interned, value, &def.paths)
            {
                let index_key = irk.to_bytes();
                let posting_key = build_posting_key(&index_key, record_id);
                ops.push(IndexWriteOp::SetPosting {
                    key: posting_key,
                    value: Bytes::new(),
                    provenance: regular_provenance(&def),
                });
            }
        }

        Ok(ops)
    }

    /// Batched version of `on_record_created`. Accepts borrowed
    /// (id, &value) pairs to avoid cloning N `InnerValue`s
    /// (`InnerValue::Map` clones its full nested structure — costly
    /// for wide records).
    ///
    /// All posting writes across all regular indexes are collected
    /// into ONE `Store::set_many` call → one backend commit on
    /// transactional backends, same number of individual writes on
    /// default-loop backends but with reduced allocation overhead.
    pub async fn on_records_created_batch<'a, R, I>(&self, items: I) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        let ops = self.plan_records_created_batch(items).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_records_created_batch` — returns
    /// accumulated `Vec<IndexWriteOp>` for all items across all
    /// regular indexes.
    pub async fn plan_records_created_batch<'a, R, I>(
        &self,
        items: I,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(1024);
        for def in self.indexes.iter() {
            let provenance = regular_provenance(&def);
            for (rid, value) in items.clone() {
                if let Some(irk) =
                    build_index_key_from_record(false, def.name_interned, value, &def.paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, rid);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
                        provenance,
                    });
                }
            }
        }

        Ok(ops)
    }

    /// Обработчик события обновления записи.
    ///
    /// Обновляет индексы при изменении записи:
    /// - Если проиндексированные поля не изменились — ничего не делает
    /// - Если изменились — удаляет старые записи индекса и добавляет новые
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор обновлённой записи
    /// * `old_value` — старое значение (до обновления)
    /// * `new_value` — новое значение (после обновления)
    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self
            .plan_record_updated(record_id, old_value, new_value)
            .await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_updated` — returns
    /// `RemovePosting` for removed values + `SetPosting` for added.
    pub async fn plan_record_updated(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(4);
        for def in self.indexes.iter() {
            let provenance = regular_provenance(&def);
            let old_key =
                build_index_key_from_record(false, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(false, def.name_interned, new_value, &def.paths);

            match (old_key, new_key) {
                (None, None) => {}
                (None, Some(nk)) => {
                    let index_key = nk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
                        provenance,
                    });
                }
                (Some(ok), None) => {
                    let index_key = ok.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::RemovePosting {
                        key: posting_key,
                        provenance,
                    });
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        let old_posting_key = build_posting_key(&old_bytes, record_id);
                        ops.push(IndexWriteOp::RemovePosting {
                            key: old_posting_key,
                            provenance,
                        });

                        let new_posting_key = build_posting_key(&new_bytes, record_id);
                        ops.push(IndexWriteOp::SetPosting {
                            key: new_posting_key,
                            value: Bytes::new(),
                            provenance,
                        });
                    }
                }
            }
        }

        Ok(ops)
    }

    /// Обработчик события удаления записи.
    ///
    /// Удаляет запись из всех активных индексов.
    /// Вызывается после успешного удаления записи из таблицы.
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор удалённой записи
    /// * `old_value` — значение удалённой записи
    pub async fn on_record_deleted(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
    ) -> DbResult<()> {
        let ops = self.plan_record_deleted(record_id, old_value).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_record_deleted` — returns
    /// `RemovePosting` for each posting of this record.
    pub async fn plan_record_deleted(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::with_capacity(4);
        for def in self.indexes.iter() {
            if let Some(irk) =
                build_index_key_from_record(false, def.name_interned, old_value, &def.paths)
            {
                let index_key = irk.to_bytes();
                let posting_key = build_posting_key(&index_key, record_id);
                ops.push(IndexWriteOp::RemovePosting {
                    key: posting_key,
                    provenance: regular_provenance(&def),
                });
            }
        }

        Ok(ops)
    }

    // ============================================================================
    // Apply ops — shared by all wrapper methods
    // ============================================================================

    /// Apply a slice of `IndexWriteOp` against `self.info_store`.
    /// Used by the `on_record_*` wrapper methods after planning.
    ///
    /// All SetPosting/RemovePosting ops are collapsed into ONE
    /// ordered `Store::transact` call — on transactional backends
    /// (sled / redb / fjall / persy / nebari / canopy) this is one
    /// atomic batch (one fsync) instead of N per-key writes. Order is
    /// preserved, so the per-key last-write-wins semantics of the
    /// original loop are unchanged. BumpFtsStats is in-memory only
    /// and not relevant for the base_index IndexManager.
    pub(super) async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value, .. } => {
                    kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
                }
                IndexWriteOp::RemovePosting { key, .. } => {
                    kv_ops.push(KvOp::Remove(key.clone().into()));
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for base_index IndexManager.
                }
            }
        }
        if !kv_ops.is_empty() {
            self.info_store.transact(kv_ops).await?;
        }
        // Invalidate posting cache for any keys touched.
        self.invalidate_posting_cache_for_ops(ops);
        Ok(())
    }

    /// Ищет записи по значению индекса.
    ///
    /// Возвращает множество RecordId, у которых проиндексированные поля
    /// соответствуют указанным значениям.
    ///
    /// # Аргументы
    ///
    /// * `name_interned` — интернированный идентификатор имени индекса
    /// * `values` — значения для поиска (должны соответствовать полям индекса)
    ///
    /// # Возвращает
    ///
    /// - `Ok(Arc<[RecordId]>)` — сортированный, дедуплицированный срез
    ///   идентификаторов записей за рефкаунт-дугой (O(1) копирование —
    ///   см. ниже). Инвариант: элементы отсортированы по `RecordId::Ord`
    ///   (байт-лексикографически) и не содержат дубликатов.
    /// - `Ok(empty slice)` — если нет записей с таким значением индекса
    /// - `Err` — ошибка чтения из хранилища
    ///
    /// # Audit 1.5 + 3.2 — Arc-срез вместо `BTreeSet`
    ///
    /// Раньше на HIT в `posting_cache` делался полный node-by-node клон
    /// дерева на КАЖДЫЙ equality-lookup (для низкокардинального индекса с
    /// 100k постингов это 100k аллокаций узлов); задача #488 заменила это
    /// на `Arc<BTreeSet<_>>` (HIT = O(1) `Arc::clone`). Задача #499 меняет
    /// само ПРЕДСТАВЛЕНИЕ: `Arc<[RecordId]>` вместо `Arc<BTreeSet<_>>`.
    ///
    /// Префикс-скан по `index_key` уже отдаёт постинги в порядке
    /// возрастания хвостовых байт `record_id` — а `RecordId::Ord` и есть
    /// байт-лексикографический порядок над `[u8; 16]`, — и ровно по одному
    /// постингу на `(index_key, record_id)`. Значит скан уже отсортирован и
    /// дедуплицирован: `BTreeSet::insert` на каждый элемент был чистыми
    /// накладными расходами (аллокация узла + ребалансировка). Теперь
    /// MISS-путь собирает `Vec<RecordId>` в порядке скана (push, без
    /// per-node аллокаций) и оборачивает в `Arc<[RecordId]>`, а потребитель
    /// итерирует непрерывный кэш-дружелюбный буфер вместо погони за
    /// указателями дерева.
    pub async fn lookup_by_index(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Arc<[RecordId]>> {
        use futures::StreamExt;

        let index_key = build_index_key(false, name_interned, values).to_bytes();

        // Opt G: try the in-memory posting cache first. DashMap's
        // sharded RwLock lets unrelated concurrent lookups proceed
        // without serialising on a single mutex.
        // Audit 1.5: cache-HIT — это атомарный refcount-bump через
        // `Arc::clone`, а НЕ полный node-by-node клон `BTreeSet`.
        if let Some(cached) = self.posting_cache.get(&index_key) {
            return Ok(Arc::clone(cached.value()));
        }

        // Scan the 25-byte index prefix; every match is a posting
        // entry whose final 16 bytes are the record_id. The scan visits
        // postings in ascending order of the trailing record_id bytes,
        // which is exactly `RecordId::Ord` (byte-lexicographic over
        // `[u8; 16]`), and there is at most one posting per record_id —
        // so `record_ids` is built already-sorted and duplicate-free by
        // plain `push`, no `BTreeSet` insert/rebalance needed (audit 3.2).
        // tunables: one-off prefix-scan batch (512); fold into a named knob under /opti.
        let mut record_ids: Vec<RecordId> = Vec::new();
        let mut stream = self.info_store.scan_prefix_stream(index_key.clone(), 512);
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                let kb: &[u8] = k.as_ref();
                if kb.len() >= index_key.len() + 16 {
                    let mut id_bytes = [0u8; 16];
                    id_bytes.copy_from_slice(&kb[index_key.len()..index_key.len() + 16]);
                    record_ids.push(RecordId(id_bytes));
                }
            }
        }

        // Populate cache (bounded — evict arbitrary entry on
        // overflow; exact LRU isn't worth the dep, index hotsets are
        // small). Empty results are cached too — the next identical
        // lookup short-circuits without re-scanning.
        //
        // Audit 1.5/3.2 (miss path): построить `Arc<[RecordId]>` ровно
        // один раз, вставить в кэш `Arc::clone` того же `Arc` и вернуть
        // исходный `Arc` вызывающему — без лишних клонов.
        if self.posting_cache.len() >= POSTING_CACHE_CAP {
            // `iter().next()` on DashMap acquires a single shard's
            // read lock — bounded; evicting an arbitrary entry is
            // explicitly allowed by the cache contract.
            if let Some(victim) = self.posting_cache.iter().next() {
                let k = victim.key().clone();
                drop(victim);
                self.posting_cache.remove(&k);
            }
        }
        let record_ids_arc: Arc<[RecordId]> = Arc::from(record_ids);
        self.posting_cache
            .insert(index_key, Arc::clone(&record_ids_arc));

        Ok(record_ids_arc)
    }

    /// Invalidate posting cache entries for every `SetPosting` /
    /// `RemovePosting` in `ops`. Called by the tx-commit pipeline
    /// (`apply_index_batch`) after durably writing the ops, so the
    /// next `lookup_by_index` re-fetches from the store.
    pub fn invalidate_posting_cache_for_ops(&self, ops: &[IndexWriteOp]) {
        for op in ops {
            let key = match op {
                IndexWriteOp::SetPosting { key, .. } | IndexWriteOp::RemovePosting { key, .. } => {
                    key
                }
                _ => continue,
            };
            if key.len() >= 25 {
                let index_key = key.slice(..25);
                self.posting_cache.remove(&index_key);
            }
        }
    }

    /// Count entries for one regular or unique index — used by the
    /// doctor's verify pass.
    pub async fn entry_count(&self, name_interned: u64, unique: bool) -> DbResult<u64> {
        use futures::StreamExt;
        let prefix = IndexRecordKey::new(unique, name_interned).to_prefix_bytes();
        let mut count: u64 = 0;
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        Ok(count)
    }

    /// Iterate over all regular index definitions.
    ///
    /// F-72 (#899, P0): NOT state-filtered — yields a `Building` definition
    /// just as readily as a `Ready` one. This is the DDL/introspection-shaped
    /// accessor (doctor `verify`/`repair`, admin DESCRIBE/LIST, DROP TABLE
    /// CASCADE, tests) that legitimately needs to see an in-flight CREATE.
    /// PLANNER call sites (anything that decides whether a query can be
    /// routed through this index) MUST use
    /// [`iter_indexes_ready`](Self::iter_indexes_ready) instead.
    pub fn iter_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes.iter()
    }

    /// Planner Ready-gate sibling of [`iter_indexes`](Self::iter_indexes):
    /// yields only `Ready` definitions, skipping any `Building` one exactly
    /// as if it did not exist yet.
    ///
    /// F-72 (#899, P0): closes the planner-invisibility gap for the regular
    /// (non-unique) hash-index family — `TableManager::find_single_field_index`
    /// and `try_plan_and_index_scan` (`read_planner.rs`) use this instead of
    /// the raw `iter_indexes`, so a concurrent Eq/In/And query cannot be
    /// planned against an index whose backfill has not yet completed (see
    /// `create_index_from_records`'s doc for the full publish/backfill/flip
    /// sequence this gates).
    pub fn iter_indexes_ready(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes
            .iter()
            .filter(|d| d.state == crate::state::IndexState::Ready)
    }

    /// Проверяет существование индекса по его имени.
    pub fn index_exists(&self, name_interned: u64) -> bool {
        self.indexes.contains(name_interned)
    }

    /// Возвращает определение индекса по его имени.
    pub fn get_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes.get_index(name_interned)
    }

    /// Re-key an in-memory regular-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// This is the metadata half of RENAME INDEX — the physical posting
    /// entries are re-keyed separately by the engine (`rekey_hash_prefix`)
    /// before this method is called. Here we only swap the in-memory
    /// `IndexDefinition` and re-save the system blob.
    ///
    /// F-8 (2026-08-06): currently unused — the live regular-index RENAME
    /// path (`TableManager::rename_index`) does drop-old+create-new instead,
    /// which bumps `generation` as a side effect of going through the normal
    /// create/drop call sites. If this method is ever wired up as a cheaper
    /// in-place rename, `bump_generation()` below is required: without it,
    /// `pre_commit.rs`'s `mgr.generation() == stage_gen` gate would not fire
    /// for a tx staged before the rename, silently skipping re-derivation.
    pub async fn rename_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes.add_index(new_def);
        self.bump_generation(); // F-8: gen gate for commit-time rederive
        self.save_index_info().await
    }

    /// Re-key an in-memory unique-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// Metadata half of RENAME INDEX for unique indexes — the physical
    /// posting entries are re-keyed separately by the engine
    /// (`rekey_hash_prefix` with `is_unique=true`).
    ///
    /// F-8 (2026-08-06): same "currently unused, bump required if ever
    /// wired up" note as [`rename_index_definition`](Self::rename_index_definition).
    pub async fn rename_unique_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes_unique.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "unique index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes_unique.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes_unique.add_index(new_def);
        self.bump_generation(); // F-8: gen gate for commit-time rederive
        self.save_index_info_unique().await
    }
}
