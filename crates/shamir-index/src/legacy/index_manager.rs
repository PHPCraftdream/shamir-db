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
use crate::legacy::index_keys::{build_index_key, build_posting_key, extract_index_leaves};
use crate::legacy::index_record_key::IndexRecordKey;
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use dashmap::DashMap;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, Store};
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
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
    /// Атомарный флаг: есть ли хоть один уникальный индекс
    pub(super) has_indexes_unique: Arc<AtomicBool>,

    /// **Opt G** — in-memory cache for posting lists. Keys are the
    /// raw physical index keys (`build_index_key(...).to_bytes()`);
    /// values are `Arc<BTreeSet<RecordId>>` — shared between hits so
    /// the lookup hot path is `HashMap::get` + `Arc::clone` + a final
    /// `BTreeSet::clone` at the boundary (caller already takes the
    /// set by value).
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
    pub(super) posting_cache: Arc<DashMap<Bytes, Arc<BTreeSet<RecordId>>, THasher>>,
}

impl Clone for IndexManager {
    fn clone(&self) -> Self {
        Self {
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            indexes_unique: Arc::clone(&self.indexes_unique),
            has_indexes: Arc::clone(&self.has_indexes),
            has_indexes_unique: Arc::clone(&self.has_indexes_unique),
            posting_cache: Arc::clone(&self.posting_cache),
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
        let indexes = match info_store.get(indexes_key.clone()).await {
            Ok(bytes) => {
                // Десериализуем метаданные; при ошибке начинаем с пустого набора
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        // Загружаем уникальные индексы или создаём пустую структуру
        let indexes_unique = match info_store.get(indexes_unique_key.clone()).await {
            Ok(bytes) => {
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
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
            has_indexes_unique: Arc::new(AtomicBool::new(has_indexes_unique_flag)),
            posting_cache: Arc::new(DashMap::with_capacity_and_hasher(
                POSTING_CACHE_CAP,
                THasher::default(),
            )),
        };

        // Синхронизируем флаги с состоянием IndexInfo
        manager.sync_flags();

        Ok(manager)
    }

    /// Синхронизирует атомарные флаги с реальным состоянием индексов.
    fn sync_flags(&self) {
        self.has_indexes
            .store(self.indexes.is_enabled(), Ordering::Release);
        self.has_indexes_unique
            .store(self.indexes_unique.is_enabled(), Ordering::Release);
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
    /// Использует атомарное чтение, поэтому очень быстро.
    /// Не требует захвата блокировки.
    pub fn has_unique_indexes(&self) -> bool {
        self.has_indexes_unique.load(Ordering::Relaxed)
    }

    /// Создаёт новый индекс для таблицы.
    ///
    /// Процесс создания:
    /// 1. Читает все существующие записи из data_store
    /// 2. Для каждой записи извлекает значения по путям индекса
    /// 3. Создаёт записи в info_store
    /// 4. Сохраняет метаданные индекса
    ///
    /// # Аргументы
    ///
    /// * `index_def` — определение индекса (имя, пути полей, уникальность)
    ///
    /// # Производительность
    ///
    /// Использует потоковую обработку (stream) с батчами по 1000 записей,
    /// чтобы избежать загрузки всех данных в память одновременно.
    pub async fn create_index(&self, index_def: IndexDefinition) -> DbResult<()> {
        use futures::StreamExt;

        // Scan data_store into a decoded vec, then delegate to the
        // shared build logic in create_index_from_records.
        let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
        let mut records: Vec<(RecordId, InnerValue)> = Vec::new();
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key_bytes, value_bytes) in batch {
                let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let record_id = RecordId(arr);
                let value = match InnerValue::from_bytes(value_bytes) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                records.push((record_id, value));
            }
        }

        self.create_index_from_records(index_def, records).await
    }

    /// FINAL-A: create index and backfill from an already-decoded record
    /// stream instead of `data_store.iter_stream`. Used by `TableManager`
    /// when an MvccStore is attached — the seam (`list_stream`) is the
    /// sole source of truth after FINAL-A.
    pub async fn create_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        let name_interned = index_def.name_interned;
        let mut count = 0usize;

        let mut posting_writes: Vec<(Bytes, Bytes)> = Vec::new();
        let mut cache_keys: Vec<(u64, Vec<InnerValue>)> = Vec::new();
        for (record_id, value) in &records {
            if let Some(values) = extract_index_leaves(value, &index_def.paths) {
                let index_key = build_index_key(false, name_interned, &values).to_bytes();
                let posting_key = build_posting_key(&index_key, record_id);
                posting_writes.push((posting_key, Bytes::new()));
                cache_keys.push((name_interned, values));
                count += 1;
            }
        }
        if !posting_writes.is_empty() {
            self.info_store.set_many(posting_writes).await?;
        }
        for (name, vals) in cache_keys {
            self.invalidate_posting_cache(name, &vals);
        }

        self.indexes.add_index(index_def);
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        log::info!(
            "Created index '{}' with {} entries (from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// Удаляет индекс по его имени.
    ///
    /// Процесс удаления:
    /// 1. Проверяет существование индекса
    /// 2. Удаляет все записи индекса из info_store (потоковая обработка)
    /// 3. Удаляет определение из метаданных
    /// 4. Сохраняет обновлённые метаданные
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

        // Формируем префикс и собираем все ключи постингов за один
        // prefix-scan, удаляем их одним вызовом `remove_many`. На
        // транзакционных backends (redb/persy/nebari) это одна
        // commit'нутая транзакция вместо N×fsync.
        let prefix = IndexRecordKey::new(false, name_interned).to_prefix_bytes();
        use futures::StreamExt;
        let mut to_remove: Vec<Bytes> = Vec::new();
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

        // Удаляем определение индекса из метаданных
        let was_removed = self.indexes.remove_index(name_interned);
        self.has_indexes
            .store(self.indexes.is_enabled(), Ordering::Release);

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
        self.info_store.set(indexes_key, Bytes::from(bytes)).await?;
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

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            if let Some(values) = extract_index_leaves(value, &def.paths) {
                let index_key = build_index_key(false, def.name_interned, &values).to_bytes();
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

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            for (rid, value) in items.clone() {
                if let Some(leaves) = extract_index_leaves(value, &def.paths) {
                    let index_key = build_index_key(false, def.name_interned, &leaves).to_bytes();
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

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            let old_values = extract_index_leaves(old_value, &def.paths);
            let new_values = extract_index_leaves(new_value, &def.paths);

            match (old_values, new_values) {
                (None, None) => {}
                (None, Some(new)) => {
                    let index_key = build_index_key(false, def.name_interned, &new).to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
                    });
                }
                (Some(old), None) => {
                    let index_key = build_index_key(false, def.name_interned, &old).to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::RemovePosting { key: posting_key });
                }
                (Some(old), Some(new)) => {
                    if old != new {
                        let old_index_key =
                            build_index_key(false, def.name_interned, &old).to_bytes();
                        let old_posting_key = build_posting_key(&old_index_key, record_id);
                        ops.push(IndexWriteOp::RemovePosting {
                            key: old_posting_key,
                        });

                        let new_index_key =
                            build_index_key(false, def.name_interned, &new).to_bytes();
                        let new_posting_key = build_posting_key(&new_index_key, record_id);
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

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            if let Some(values) = extract_index_leaves(old_value, &def.paths) {
                let index_key = build_index_key(false, def.name_interned, &values).to_bytes();
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
                    kv_ops.push(KvOp::Set(key.clone(), value.clone()));
                }
                IndexWriteOp::RemovePosting { key } => {
                    kv_ops.push(KvOp::Remove(key.clone()));
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
        // We invalidate broadly: any SetPosting/RemovePosting key whose
        // first 25 bytes match a cached index_key prefix.
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
    /// - `Ok(BTreeSet<RecordId>)` — множество идентификаторов записей
    /// - `Ok(empty)` — если нет записей с таким значением индекса
    /// - `Err` — ошибка чтения из хранилища
    pub async fn lookup_by_index(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        use futures::StreamExt;

        let index_key = build_index_key(false, name_interned, values).to_bytes();

        // Opt G: try the in-memory posting cache first. DashMap's
        // sharded RwLock lets unrelated concurrent lookups proceed
        // without serialising on a single mutex.
        if let Some(cached) = self.posting_cache.get(&index_key) {
            return Ok((**cached).clone());
        }

        // Scan the 25-byte index prefix; every match is a posting
        // entry whose final 16 bytes are the record_id.
        // tunables: one-off prefix-scan batch (512); fold into a named knob under /opti.
        let mut record_ids: BTreeSet<RecordId> = BTreeSet::new();
        let mut stream = self.info_store.scan_prefix_stream(index_key.clone(), 512);
        while let Some(batch) = stream.next().await {
            for (k, _) in batch? {
                let kb: &[u8] = k.as_ref();
                if kb.len() >= index_key.len() + 16 {
                    let mut id_bytes = [0u8; 16];
                    id_bytes.copy_from_slice(&kb[index_key.len()..index_key.len() + 16]);
                    record_ids.insert(RecordId(id_bytes));
                }
            }
        }

        // Populate cache (bounded — evict arbitrary entry on
        // overflow; exact LRU isn't worth the dep, index hotsets are
        // small). Empty results are cached too — the next identical
        // lookup short-circuits without re-scanning.
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
        self.posting_cache
            .insert(index_key, Arc::new(record_ids.clone()));

        Ok(record_ids)
    }

    /// Invalidate any cached posting list for `(name_interned,
    /// values)`. Called from write hooks after the durable update
    /// landed, so the next `lookup_by_index` re-fetches.
    fn invalidate_posting_cache(&self, name_interned: u64, values: &[InnerValue]) {
        let index_key = build_index_key(false, name_interned, values).to_bytes();
        self.posting_cache.remove(&index_key);
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
    pub fn iter_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes.iter()
    }

    /// Проверяет существование индекса по его имени.
    pub fn index_exists(&self, name_interned: u64) -> bool {
        self.indexes.contains(name_interned)
    }

    /// Возвращает определение индекса по его имени.
    pub fn get_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes.get_index(name_interned)
    }
}
