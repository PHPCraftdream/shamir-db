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

use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info::IndexInfo;
use crate::legacy::index_keys::{build_index_key, build_index_key_from_record, build_posting_key};
use crate::legacy::index_record_key::IndexRecordKey;
use crate::legacy::write_barrier_flags::{WriteBarrierFlags, UNIQUE_INDEX_EXISTS};
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use dashmap::DashMap;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Maximum number of posting-list entries cached in memory per
/// `IndexManager`. Hit on a cached entry is a single `HashMap::get`
/// + `Arc::clone`; miss falls back to `info_store.get` + bincode
///   deserialise. Capacity is intentionally small — typical workloads
///   (admin UIs, filter-by-status, find-by-id) concentrate on a handful
///   of values per index.
const POSTING_CACHE_CAP: usize = 512;

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
    /// crate's own `cfg(test)` is inactive — see `legacy/mod.rs`'s module
    /// doc on `backfill_pause_hook` for the cross-crate-visibility reason.
    pub(super) create_index_backfill_hook:
        Arc<arc_swap::ArcSwapOption<crate::legacy::backfill_pause_hook::BackfillPauseHook>>,
    /// F-76 (#903) test-only deterministic pause point: parks `drop_index`
    /// (regular) AND `drop_unique_index` (unique) between the definition
    /// retirement and the posting sweep — the exact visibility window this
    /// task closes (the mirror image of F-72's CREATE bug). A regression
    /// test installs this from `shamir-engine`'s test binary, so (like
    /// `create_index_backfill_hook`) it is NOT `#[cfg(test)]`-gated. Shared
    /// by both legacy DROP paths — a test exercises only one at a time.
    /// `None` on every real path; one uncontended `ArcSwapOption::load_full()`
    /// Acquire load at the call site. See `f76_drop_visibility_tests.rs`.
    pub(super) drop_index_pause_hook:
        Arc<arc_swap::ArcSwapOption<crate::legacy::backfill_pause_hook::BackfillPauseHook>>,
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
        };

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
        hook: Option<Arc<crate::legacy::backfill_pause_hook::BackfillPauseHook>>,
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
        hook: Option<Arc<crate::legacy::backfill_pause_hook::BackfillPauseHook>>,
    ) {
        self.drop_index_pause_hook.store(hook);
    }

    /// Синхронизирует атомарные флаги с реальным состоянием индексов.
    fn sync_flags(&self) {
        self.has_indexes
            .store(self.indexes.is_enabled(), Ordering::Release);
        self.write_barrier_flags
            .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
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
        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST ──────────────────────────
        // (Same concurrency rationale as `create_index_from_records`.)
        self.indexes.add_index(index_def);
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
        let mut count = 0usize;
        let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let mut posting_writes: Vec<(Bytes, Bytes)> =
                Vec::with_capacity(batch.len().min(131_072));
            let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(posting_writes.capacity());
            for (key_bytes, value_bytes) in &batch {
                let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let record_id = RecordId(arr);
                let value = match InnerValue::from_bytes(value_bytes) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
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
    /// Unlike index2, the legacy `IndexManager` family has NO automatic
    /// restart-from-scratch self-heal for a `Building` definition at
    /// table-open time (grep-verified: `IndexManager::new` only loads
    /// definitions via `IndexInfo::decode_bytes`, it does not re-run any
    /// backfill). A `Building` definition left behind by a crash/error
    /// therefore stays durably `Building` — permanently planner-invisible,
    /// never silently resurrected as `Ready` — until an operator runs
    /// `TableManager::repair()` (`doctor.rs`), which rebuilds every
    /// definition unconditionally regardless of state. This is an accepted,
    /// explicitly-documented gap: automatic legacy-family self-healing is
    /// out of scope for this task (mirrors the equivalent note on
    /// `create_sorted_index_with_include`).
    pub async fn create_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        let name_interned = index_def.name_interned;
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
            self.info_store.set_many(posting_writes).await?;
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
        self.save_index_info().await?;

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
        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST, at Building ────────────
        // (Identical to `create_index_from_records` — see that method's doc.)
        self.indexes.add_index(index_def);
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        // ── Phase 2: stream + batch-write postings (F-78 body) ──────────────
        // O(batch) peak memory: each batch's posting_writes / cache_index_keys
        // are built, flushed via `set_many`, and dropped before the next batch.
        // Idempotent against a record also written by the live hook at the
        // registration boundary — same (posting_key, empty-value) pair.
        let mut count = 0usize;
        let mut stream = stream;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
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
                self.info_store.set_many(posting_writes).await?;
            }
            for ik in cache_index_keys {
                self.posting_cache.remove(&ik);
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
        self.save_index_info().await?;

        log::info!(
            "Created index '{}' with {} entries (streamed from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// Удаляет индекс по его имени.
    ///
    /// Процесс удаления (F-76 / #903 — definition retired BEFORE the posting
    /// sweep, the mirror image of F-72's CREATE gate):
    /// 1. Проверяет существование индекса
    /// 2. Удаляет определение из метаданных (planner-invisible с этого
    ///    момента — RCU-свап публикует Vec без этой дефиниции)
    /// 3. Удаляет все записи индекса из info_store (теперь orphan,
    ///    planner-invisible)
    /// 4. Сохраняет обновлённые метаданные
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
    /// # Возвращает
    ///
    /// `true` — индекс существовал и был удалён
    /// `false` — индекс не найден
    pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
        // Быстрая проверка существования индекса
        if !self.indexes.contains(name_interned) {
            return Ok(false);
        }

        let prefix = IndexRecordKey::new(false, name_interned).to_prefix_bytes();

        // F-76 (#903): retire the definition from the planner-visible Vec
        // FIRST (see the method doc). The RCU swap publishes a Vec without
        // this definition atomically.
        let was_removed = self.indexes.remove_index(name_interned);
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

        // Sweep the (now orphan, planner-invisible) posting entries. Формируем
        // префикс и собираем все ключи постингов за один prefix-scan, удаляем
        // их одним вызовом `remove_many`. На транзакционных backends
        // (redb/persy/nebari) это одна commit'нутая транзакция вместо N×fsync.
        use futures::StreamExt;
        // `RecordKey` (the scan yields store keys, consumed by `remove_many`).
        let mut to_remove: Vec<RecordKey> = Vec::new();
        // tunables: prefix scan currently uses FULL_SCAN_BATCH(1000); profile is arguably MAINT(256) — revisit under /opti.
        let mut stream = self
            .info_store
            .scan_prefix_stream(prefix.clone(), FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            for (key, _) in batch_result? {
                to_remove.push(key);
            }
        }
        if !to_remove.is_empty() {
            // Ok-value (removed entries) intentionally discarded; ? propagates errors.
            let _ = self.info_store.remove_many(to_remove).await?;
        }

        // Posting cache: blow away every entry whose key starts
        // with the index's prefix. Cheap — typical hotsets are
        // small and the cache is bounded.
        self.posting_cache.retain(|k, _| !k.starts_with(&prefix));

        if was_removed {
            self.save_index_info().await?;
        }

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
            for (rid, value) in items.clone() {
                if let Some(irk) =
                    build_index_key_from_record(false, def.name_interned, value, &def.paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, rid);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
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
                    });
                }
                (Some(ok), None) => {
                    let index_key = ok.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::RemovePosting { key: posting_key });
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        let old_posting_key = build_posting_key(&old_bytes, record_id);
                        ops.push(IndexWriteOp::RemovePosting {
                            key: old_posting_key,
                        });

                        let new_posting_key = build_posting_key(&new_bytes, record_id);
                        ops.push(IndexWriteOp::SetPosting {
                            key: new_posting_key,
                            value: Bytes::new(),
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
                ops.push(IndexWriteOp::RemovePosting { key: posting_key });
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
    /// and not relevant for the legacy IndexManager.
    pub(super) async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
        let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
        for op in ops {
            match op {
                IndexWriteOp::SetPosting { key, value } => {
                    kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
                }
                IndexWriteOp::RemovePosting { key } => {
                    kv_ops.push(KvOp::Remove(key.clone().into()));
                }
                IndexWriteOp::BumpFtsStats { .. } => {
                    // Not relevant for legacy IndexManager.
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
                IndexWriteOp::SetPosting { key, .. } | IndexWriteOp::RemovePosting { key } => key,
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
    pub async fn rename_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes.add_index(new_def);
        self.save_index_info().await
    }

    /// Re-key an in-memory unique-index definition from `old_id` to `new_id`
    /// and persist the updated metadata.
    ///
    /// Metadata half of RENAME INDEX for unique indexes — the physical
    /// posting entries are re-keyed separately by the engine
    /// (`rekey_hash_prefix` with `is_unique=true`).
    pub async fn rename_unique_index_definition(&self, old_id: u64, new_id: u64) -> DbResult<()> {
        let old_def = self.indexes_unique.get_index(old_id).ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "unique index definition disappeared mid-rename".to_string(),
            )
        })?;
        self.indexes_unique.remove_index(old_id);
        let new_def = IndexDefinition::new(new_id, old_def.paths.clone());
        self.indexes_unique.add_index(new_def);
        self.save_index_info_unique().await
    }
}
