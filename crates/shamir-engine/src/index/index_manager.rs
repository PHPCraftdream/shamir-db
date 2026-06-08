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

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info::IndexInfo;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_record_key::IndexRecordKey;
use crate::index2::write_ops::IndexWriteOp;
use crate::meta::MetaKey;
use bytes::Bytes;
use dashmap::DashMap;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, Store};
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::core::interner::InternerKey;
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
    data_store: Arc<dyn Store>,
    /// Служебное хранилище для метаданных и записей индексов
    info_store: Arc<dyn Store>,

    /// Метаданные обычных индексов (неуникальных)
    /// IndexInfo использует DashMap внутри, поэтому thread-safe без дополнительной синхронизации
    indexes: Arc<IndexInfo>,
    /// Метаданные уникальных индексов
    indexes_unique: Arc<IndexInfo>,

    /// Атомарный флаг: есть ли хоть один обычный индекс
    /// Arc позволяет всем клонам видеть одно и то же состояние
    has_indexes: Arc<AtomicBool>,
    /// Атомарный флаг: есть ли хоть один уникальный индекс
    has_indexes_unique: Arc<AtomicBool>,

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
    posting_cache: Arc<DashMap<Bytes, Arc<BTreeSet<RecordId>>>>,
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
        let indexes_key = MetaKey::LegacyIndexes.as_record_id().to_bytes();
        let indexes_unique_key = MetaKey::LegacyIndexesUnique.as_record_id().to_bytes();

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
            posting_cache: Arc::new(DashMap::with_capacity(POSTING_CACHE_CAP)),
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

    /// Извлекает значение из InnerValue по пути (список интернированных ключей).
    ///
    /// Путь представляет собой последовательность ключей для навигации
    /// по вложенным структурам данных (Map).
    ///
    /// # Аргументы
    ///
    /// * `value` — исходное значение (обычно Map)
    /// * `path` — путь к искомому полю (интернированные ключи)
    ///
    /// # Пример
    ///
    /// Для JSON `{"user": {"name": "John"}}` путь к имени будет `[key_user, key_name]`.
    fn extract_value_by_path(value: &InnerValue, path: &[u64]) -> Option<InnerValue> {
        Self::extract_value_by_path_ref(value, path).cloned()
    }

    /// Borrowing variant of `extract_value_by_path` — walks the
    /// record in-place without cloning the leaf. Hot batch write
    /// paths (build_index_key needs only `&InnerValue` to feed into
    /// FxHash) use this; the owned variant survives for the few
    /// callers that genuinely consume the value.
    fn extract_value_by_path_ref<'a>(
        value: &'a InnerValue,
        path: &[u64],
    ) -> Option<&'a InnerValue> {
        let mut cur = value;
        for &id in path {
            match cur {
                InnerValue::Map(map) => {
                    let key = InternerKey::new(id);
                    cur = map.get(&key)?;
                }
                _ => return None,
            }
        }
        Some(cur)
    }

    /// Извлекает значения для составного индекса из записи.
    ///
    /// Составной индекс может включать несколько полей (например, [name, age]).
    /// Этот метод извлекает все указанные поля и возвращает их как вектор.
    ///
    /// # Возвращает
    ///
    /// - `Some(Vec<InnerValue>)` — все поля успешно извлечены
    /// - `None` — хотя бы одно поле отсутствует
    pub fn extract_index_values(
        value: &InnerValue,
        paths: &[IndexInfoItem],
    ) -> Option<Vec<InnerValue>> {
        let mut values = Vec::with_capacity(paths.len());

        // Извлекаем значение для каждого поля индекса
        for item in paths {
            match Self::extract_value_by_path(value, &item.path) {
                Some(v) => values.push(v),
                None => return None, // Если хоть одно поле отсутствует — индекс не применим
            }
        }
        Some(values)
    }

    /// Borrowing variant of `extract_index_values` — returns
    /// `Vec<&InnerValue>` (no per-leaf clone). Used by batch write
    /// hot paths that immediately feed the values into
    /// `IndexRecordKey::with_values` (which takes `&[&InnerValue]`
    /// already) and into `build_posting_key`.
    pub fn extract_index_values_ref<'a>(
        value: &'a InnerValue,
        paths: &[IndexInfoItem],
    ) -> Option<Vec<&'a InnerValue>> {
        let mut values = Vec::with_capacity(paths.len());
        for item in paths {
            match Self::extract_value_by_path_ref(value, &item.path) {
                Some(v) => values.push(v),
                None => return None,
            }
        }
        Some(values)
    }

    /// Build the 25-byte index key from already-borrowed value refs.
    /// Used by the batch write paths so they don't allocate an owned
    /// `Vec<InnerValue>` just to feed the hash function.
    fn build_index_key_from_refs(
        is_unique: bool,
        name_interned: u64,
        value_refs: &[&InnerValue],
    ) -> IndexRecordKey {
        IndexRecordKey::new(is_unique, name_interned).with_values(value_refs)
    }

    /// Строит ключ записи индекса.
    ///
    /// Ключ индекса состоит из:
    /// - Флага is_unique (1 байт)
    /// - Идентификатора индекса (интернированное имя, 8 байт)
    /// - Хешей значений проиндексированных полей (16 байт)
    ///
    /// Это позволяет быстро находить записи по значению индексируемых полей.
    fn build_index_key(
        is_unique: bool,
        name_interned: u64,
        values: &[InnerValue],
    ) -> IndexRecordKey {
        let value_refs: Vec<&InnerValue> = values.iter().collect();
        IndexRecordKey::new(is_unique, name_interned).with_values(&value_refs)
    }

    /// Compose the physical posting key:
    /// `index_key (25b) || record_id (16b)` = 41 bytes.
    fn build_posting_key(index_key: &Bytes, record_id: &RecordId) -> Bytes {
        let mut k = Vec::with_capacity(index_key.len() + 16);
        k.extend_from_slice(index_key);
        k.extend_from_slice(record_id.as_bytes());
        Bytes::from(k)
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
            if let Some(values) = Self::extract_index_values(value, &index_def.paths) {
                let index_key = Self::build_index_key(false, name_interned, &values).to_bytes();
                let posting_key = Self::build_posting_key(&index_key, record_id);
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
    async fn save_index_info(&self) -> DbResult<()> {
        let indexes_key = MetaKey::LegacyIndexes.as_record_id().to_bytes();
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
        value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                let index_key = Self::build_index_key(false, def.name_interned, &values).to_bytes();
                let posting_key = Self::build_posting_key(&index_key, record_id);
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
    pub async fn on_records_created_batch<'a, I>(&self, items: I) -> DbResult<()>
    where
        I: IntoIterator<Item = (&'a RecordId, &'a InnerValue)> + Clone,
    {
        let ops = self.plan_records_created_batch(items).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_records_created_batch` — returns
    /// accumulated `Vec<IndexWriteOp>` for all items across all
    /// regular indexes.
    pub async fn plan_records_created_batch<'a, I>(&self, items: I) -> DbResult<Vec<IndexWriteOp>>
    where
        I: IntoIterator<Item = (&'a RecordId, &'a InnerValue)> + Clone,
    {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            for (rid, value) in items.clone() {
                if let Some(value_refs) = Self::extract_index_values_ref(value, &def.paths) {
                    let index_key =
                        Self::build_index_key_from_refs(false, def.name_interned, &value_refs)
                            .to_bytes();
                    let posting_key = Self::build_posting_key(&index_key, rid);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
                    });
                }
            }
        }

        Ok(ops)
    }

    /// Batched version of `on_record_created_unique`. Same borrow
    /// shape as `on_records_created_batch`.
    pub async fn on_records_created_unique_batch<'a, I>(&self, items: I) -> DbResult<()>
    where
        I: IntoIterator<Item = (&'a RecordId, &'a InnerValue)> + Clone,
    {
        let ops = self.plan_records_created_unique_batch(items).await?;
        self.apply_ops(&ops).await
    }

    /// Planner variant of `on_records_created_unique_batch` — returns
    /// `Vec<IndexWriteOp>`. Uniqueness validation (collision detection)
    /// stays in the plan phase: it reads existing postings to detect
    /// duplicates. If collision → `Err(DuplicateKey(...))`.
    pub async fn plan_records_created_unique_batch<'a, I>(
        &self,
        items: I,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        I: IntoIterator<Item = (&'a RecordId, &'a InnerValue)> + Clone,
    {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            for (rid, value) in items.clone() {
                if let Some(value_refs) = Self::extract_index_values_ref(value, &def.paths) {
                    let index_key =
                        Self::build_index_key_from_refs(true, def.name_interned, &value_refs)
                            .to_bytes();
                    ops.push(IndexWriteOp::SetPosting {
                        key: index_key,
                        value: Bytes::copy_from_slice(rid.as_bytes()),
                    });
                }
            }
        }
        Ok(ops)
    }

    /// Single-record planner for unique-index postings on create.
    ///
    /// Mirrors [`plan_records_created_unique_batch`] for the one-record
    /// case (the tx insert path). Emits one
    /// `SetPosting { key: index_key (25b), value: record_id }` per unique
    /// index whose paths the record satisfies — the exact physical layout
    /// `add_unique_entry` / `check_unique_constraint` read back.
    ///
    /// Does NOT validate uniqueness — the caller must run
    /// [`validate_unique_for_create`] first (at stage time, under the tx
    /// staging path). See the tx-concurrent unique gap documented on
    /// `TableManager::insert_tx`.
    pub async fn plan_record_created_unique(
        &self,
        record_id: &RecordId,
        value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                let index_key = Self::build_index_key(true, def.name_interned, &values).to_bytes();
                ops.push(IndexWriteOp::SetPosting {
                    key: index_key,
                    value: Bytes::copy_from_slice(record_id.as_bytes()),
                });
            }
        }
        Ok(ops)
    }

    /// Single-record planner for unique-index posting changes on update.
    ///
    /// Mirrors [`on_record_updated_unique`] as a planner: for each unique
    /// index, remove the old `(value)` posting and set the new one when
    /// the indexed value changed. Does NOT validate — caller runs
    /// [`validate_unique_for_update`] first.
    pub async fn plan_record_updated_unique(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            let old_values = Self::extract_index_values(old_value, &def.paths);
            let new_values = Self::extract_index_values(new_value, &def.paths);
            match (old_values, new_values) {
                (None, None) => {}
                (None, Some(new)) => {
                    let key = Self::build_index_key(true, def.name_interned, &new).to_bytes();
                    ops.push(IndexWriteOp::SetPosting {
                        key,
                        value: Bytes::copy_from_slice(record_id.as_bytes()),
                    });
                }
                (Some(old), None) => {
                    let key = Self::build_index_key(true, def.name_interned, &old).to_bytes();
                    ops.push(IndexWriteOp::RemovePosting { key });
                }
                (Some(old), Some(new)) => {
                    if old != new {
                        let old_key =
                            Self::build_index_key(true, def.name_interned, &old).to_bytes();
                        ops.push(IndexWriteOp::RemovePosting { key: old_key });
                        let new_key =
                            Self::build_index_key(true, def.name_interned, &new).to_bytes();
                        ops.push(IndexWriteOp::SetPosting {
                            key: new_key,
                            value: Bytes::copy_from_slice(record_id.as_bytes()),
                        });
                    }
                }
            }
        }
        Ok(ops)
    }

    /// Single-record planner for unique-index posting removals on delete.
    ///
    /// Mirrors [`on_record_deleted_unique`] as a planner.
    pub async fn plan_record_deleted_unique(
        &self,
        _record_id: &RecordId,
        old_value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(values) = Self::extract_index_values(old_value, &def.paths) {
                let key = Self::build_index_key(true, def.name_interned, &values).to_bytes();
                ops.push(IndexWriteOp::RemovePosting { key });
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
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            let old_values = Self::extract_index_values(old_value, &def.paths);
            let new_values = Self::extract_index_values(new_value, &def.paths);

            match (old_values, new_values) {
                (None, None) => {}
                (None, Some(new)) => {
                    let index_key =
                        Self::build_index_key(false, def.name_interned, &new).to_bytes();
                    let posting_key = Self::build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::SetPosting {
                        key: posting_key,
                        value: Bytes::new(),
                    });
                }
                (Some(old), None) => {
                    let index_key =
                        Self::build_index_key(false, def.name_interned, &old).to_bytes();
                    let posting_key = Self::build_posting_key(&index_key, record_id);
                    ops.push(IndexWriteOp::RemovePosting { key: posting_key });
                }
                (Some(old), Some(new)) => {
                    if old != new {
                        let old_index_key =
                            Self::build_index_key(false, def.name_interned, &old).to_bytes();
                        let old_posting_key = Self::build_posting_key(&old_index_key, record_id);
                        ops.push(IndexWriteOp::RemovePosting {
                            key: old_posting_key,
                        });

                        let new_index_key =
                            Self::build_index_key(false, def.name_interned, &new).to_bytes();
                        let new_posting_key = Self::build_posting_key(&new_index_key, record_id);
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
        old_value: &InnerValue,
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_indexes() {
            return Ok(Vec::new());
        }

        let mut ops = Vec::new();
        for def in self.indexes.iter() {
            if let Some(values) = Self::extract_index_values(old_value, &def.paths) {
                let index_key = Self::build_index_key(false, def.name_interned, &values).to_bytes();
                let posting_key = Self::build_posting_key(&index_key, record_id);
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
    async fn apply_ops(&self, ops: &[IndexWriteOp]) -> DbResult<()> {
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

        let index_key = Self::build_index_key(false, name_interned, values).to_bytes();

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
        let index_key = Self::build_index_key(false, name_interned, values).to_bytes();
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

    // ============================================================================
    // UNIQUE INDEXES - Validation (BEFORE write)
    // ============================================================================

    /// Проверяет уникальность перед созданием записи.
    ///
    /// Должен вызываться ДО записи в таблицу.
    /// Возвращает `Err(DuplicateKey)` если хотя бы один уникальный индекс нарушен.
    ///
    /// # Аргументы
    ///
    /// * `value` — значение новой записи
    pub async fn validate_unique_for_create(&self, value: &InnerValue) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                if let Some(existing_id) = self
                    .check_unique_constraint(def.name_interned, &values)
                    .await?
                {
                    return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                        "Unique index '{}' violated: value already exists for record {:?}",
                        def.name_interned, existing_id
                    )));
                }
            }
        }

        Ok(())
    }

    /// Проверяет уникальность перед обновлением записи.
    ///
    /// Должен вызываться ДО записи в таблицу.
    /// Возвращает `Err(DuplicateKey)` если хотя бы один уникальный индекс нарушен.
    /// Исключает саму обновляемую запись из проверки.
    ///
    /// # Аргументы
    ///
    /// * `record_id` — идентификатор обновляемой записи
    /// * `old_value` — старое значение (до обновления)
    /// * `new_value` — новое значение (после обновления)
    pub async fn validate_unique_for_update(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            let old_values = Self::extract_index_values(old_value, &def.paths);
            let new_values = Self::extract_index_values(new_value, &def.paths);

            // Если значение не изменилось или оба отсутствуют — пропускаем
            match (&old_values, &new_values) {
                (None, None) => continue,
                (Some(old), Some(new)) if old == new => continue,
                _ => {}
            }

            // Проверяем новое значение (если оно есть)
            if let Some(new) = &new_values {
                if let Some(existing_id) =
                    self.check_unique_constraint(def.name_interned, new).await?
                {
                    // Если существующая запись — это не мы сами, то нарушение
                    if &existing_id != record_id {
                        return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                            "Unique index '{}' violated: value already exists for record {:?}",
                            def.name_interned, existing_id
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Deterministic unique-index keys this `value` would claim.
    ///
    /// For every unique index whose paths the value fully populates,
    /// returns the BYTE-IDENTICAL key that `check_unique_constraint`
    /// (and `add_unique_entry`) read/write — i.e.
    /// `build_index_key(true, name_interned, values).to_bytes()`. The
    /// tx commit path records these as `UniqueGuard`s and re-validates
    /// them under `commit_lock`, closing the tx-concurrent unique hole.
    ///
    /// Returns an empty vec when there are no unique indexes or the
    /// value populates none of them.
    pub fn unique_keys_for(&self, value: &InnerValue) -> Vec<Bytes> {
        if !self.has_unique_indexes() {
            return Vec::new();
        }
        let mut keys = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                keys.push(Self::build_index_key(true, def.name_interned, &values).to_bytes());
            }
        }
        keys
    }

    /// Проверяет, существует ли запись с данным значением в уникальном индексе.
    ///
    /// # Возвращает
    ///
    /// - `Ok(Some(RecordId))` — запись существует
    /// - `Ok(None)` — значение свободно
    /// - `Err` — ошибка чтения
    async fn check_unique_constraint(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Option<RecordId>> {
        let index_key = Self::build_index_key(true, name_interned, values).to_bytes();

        match self.info_store.get(index_key).await {
            Ok(bytes) => {
                if bytes.len() == 16 {
                    let arr: [u8; 16] = bytes.as_ref().try_into().unwrap();
                    Ok(Some(RecordId(arr)))
                } else {
                    // Коррупция данных — считаем что занято
                    log::warn!("Invalid unique index value length: {}", bytes.len());
                    Ok(None)
                }
            }
            Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ============================================================================
    // UNIQUE INDEXES - Storage helpers
    // ============================================================================

    /// Добавляет запись в уникальный индекс.
    ///
    /// Ключ: `[index_key with is_unique=true]` (25 байт)
    /// Значение: `RecordId` (16 байт)
    ///
    /// # Важно
    ///
    /// Не проверяет уникальность! Вызывай `validate_unique_*` перед этим методом.
    async fn add_unique_entry(
        &self,
        name_interned: u64,
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> DbResult<()> {
        let index_key = Self::build_index_key(true, name_interned, values).to_bytes();
        self.info_store.set(index_key, record_id.to_bytes()).await?;
        Ok(())
    }

    /// Удаляет запись из уникального индекса.
    async fn remove_unique_entry(&self, name_interned: u64, values: &[InnerValue]) -> DbResult<()> {
        let index_key = Self::build_index_key(true, name_interned, values).to_bytes();
        self.info_store.remove(index_key).await?;
        Ok(())
    }

    // ============================================================================
    // UNIQUE INDEXES - Event handlers (AFTER write)
    // ============================================================================

    /// Обработчик события создания записи для уникальных индексов.
    ///
    /// Добавляет новую запись во все уникальные индексы.
    /// Вызывается ПОСЛЕ успешной вставки записи в таблицу.
    ///
    /// # Важно
    ///
    /// Перед вызовом должна быть выполнена валидация через `validate_unique_for_create`!
    pub async fn on_record_created_unique(
        &self,
        record_id: &RecordId,
        value: &InnerValue,
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                self.add_unique_entry(def.name_interned, &values, record_id)
                    .await?;
            }
        }

        Ok(())
    }

    /// Обработчик события обновления записи для уникальных индексов.
    ///
    /// Обновляет уникальные индексы при изменении записи.
    /// Вызывается ПОСЛЕ успешного обновления записи в таблице.
    ///
    /// # Важно
    ///
    /// Перед вызовом должна быть выполнена валидация через `validate_unique_for_update`!
    pub async fn on_record_updated_unique(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            let old_values = Self::extract_index_values(old_value, &def.paths);
            let new_values = Self::extract_index_values(new_value, &def.paths);

            match (old_values, new_values) {
                (None, None) => {}
                (None, Some(new)) => {
                    self.add_unique_entry(def.name_interned, &new, record_id)
                        .await?;
                }
                (Some(old), None) => {
                    self.remove_unique_entry(def.name_interned, &old).await?;
                }
                (Some(old), Some(new)) => {
                    if old != new {
                        self.remove_unique_entry(def.name_interned, &old).await?;
                        self.add_unique_entry(def.name_interned, &new, record_id)
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Обработчик события удаления записи для уникальных индексов.
    ///
    /// Удаляет запись из всех уникальных индексов.
    /// Вызывается ПОСЛЕ успешного удаления записи из таблицы.
    pub async fn on_record_deleted_unique(
        &self,
        _record_id: &RecordId,
        old_value: &InnerValue,
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(values) = Self::extract_index_values(old_value, &def.paths) {
                self.remove_unique_entry(def.name_interned, &values).await?;
            }
        }

        Ok(())
    }

    // ============================================================================
    // UNIQUE INDEXES - Management
    // ============================================================================

    /// Создаёт новый уникальный индекс для таблицы.
    ///
    /// Процесс создания:
    /// 1. Проверяет уникальность всех существующих значений
    /// 2. Если есть дубликаты — возвращает ошибку с количеством дубликатов
    /// 3. Иначе создаёт индекс
    ///
    /// # Возвращает
    ///
    /// - `Ok(())` — индекс успешно создан
    /// - `Err(UniqueIndexCreationFailed)` — найдены дубликаты, содержит:
    ///   - имя индекса
    ///   - количество записей с дублирующимися значениями
    ///   - пример дублирующегося значения
    pub async fn create_unique_index(&self, index_def: IndexDefinition) -> DbResult<()> {
        use futures::StreamExt;

        // Scan data_store into a decoded vec, then delegate to the
        // shared build logic in create_unique_index_from_records.
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

        self.create_unique_index_from_records(index_def, records)
            .await
    }

    /// FINAL-A: create unique index and backfill from pre-decoded records
    /// instead of `data_store.iter_stream`. Used by `TableManager` when an
    /// MvccStore is attached.
    pub async fn create_unique_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        use shamir_types::types::common::{new_map, TMap};

        let name_interned = index_def.name_interned;
        let mut value_counts: TMap<Vec<u8>, usize> = new_map();
        let mut entries: Vec<(RecordId, Vec<u8>, Vec<InnerValue>)> = Vec::new();

        for (record_id, value) in &records {
            if let Some(values) = Self::extract_index_values(value, &index_def.paths) {
                let values_key = bincode::serialize(&values)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                *value_counts.entry(values_key.clone()).or_insert(0) += 1;
                entries.push((*record_id, values_key, values));
            }
        }

        let duplicates: Vec<(&Vec<u8>, &usize)> =
            value_counts.iter().filter(|(_, &c)| c > 1).collect();
        if !duplicates.is_empty() {
            let duplicate_record_count: usize = duplicates.iter().map(|(_, &c)| c).sum();
            let sample_key = duplicates[0].0;
            let sample_values: Vec<InnerValue> =
                bincode::deserialize(sample_key).unwrap_or_else(|_| vec![InnerValue::Null]);
            let sample_str = Self::format_values_for_error(&sample_values);
            return Err(shamir_storage::error::DbError::UniqueIndexCreationFailed(
                name_interned.to_string(),
                duplicate_record_count,
                sample_str,
            ));
        }

        let count = entries.len();
        let mut writes: Vec<(Bytes, Bytes)> = Vec::with_capacity(count);
        for (record_id, _values_key, values) in entries {
            let index_key = Self::build_index_key(true, name_interned, &values).to_bytes();
            writes.push((index_key, Bytes::copy_from_slice(record_id.as_bytes())));
        }
        if !writes.is_empty() {
            self.info_store.set_many(writes).await?;
        }

        self.indexes_unique.add_index(index_def);
        self.has_indexes_unique.store(true, Ordering::Release);
        self.save_index_info_unique().await?;

        log::info!(
            "Created unique index '{}' with {} entries (from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// Форматирует значения для сообщения об ошибке.
    fn format_values_for_error(values: &[InnerValue]) -> String {
        let formatted: Vec<String> = values
            .iter()
            .map(|v| match v {
                InnerValue::Null => "null".to_string(),
                InnerValue::Bool(b) => b.to_string(),
                InnerValue::Int(n) => n.to_string(),
                InnerValue::F64(n) => n.to_string(),
                InnerValue::Dec(d) => d.to_string(),
                InnerValue::Big(b) => b.to_string(),
                InnerValue::Str(s) => format!("\"{}\"", s),
                InnerValue::Bin(_) => "<binary>".to_string(),
                InnerValue::List(arr) => {
                    if arr.len() <= 5 {
                        format!("[{}]", arr.len())
                    } else {
                        format!("[{}...]", arr.len())
                    }
                }
                InnerValue::Set(s) => format!("{{{} items}}", s.len()),
                InnerValue::Map(map) => format!("{{{} fields}}", map.len()),
            })
            .collect();
        formatted.join(", ")
    }

    /// Удаляет уникальный индекс по его имени.
    ///
    /// # Возвращает
    ///
    /// `true` — индекс существовал и был удалён
    /// `false` — индекс не найден
    pub async fn drop_unique_index(&self, name_interned: u64) -> DbResult<bool> {
        if !self.indexes_unique.contains(name_interned) {
            return Ok(false);
        }

        // Формируем префикс и удаляем все posting-ключи одним
        // вызовом `remove_many` — на disk backends это один
        // транзакционный коммит вместо N×fsync.
        let prefix = IndexRecordKey::new(true, name_interned).to_prefix_bytes();
        use futures::StreamExt;
        let mut to_remove: Vec<Bytes> = Vec::new();
        // tunables: prefix scan currently uses FULL_SCAN_BATCH(1000); profile is arguably MAINT(256) — revisit under /opti.
        let mut stream = self.info_store.scan_prefix_stream(prefix, FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            for (key, _) in batch_result? {
                to_remove.push(key);
            }
        }
        if !to_remove.is_empty() {
            // Ok-value (removed entries) intentionally discarded; ? propagates errors.
            let _ = self.info_store.remove_many(to_remove).await?;
        }

        // Удаляем определение индекса из метаданных
        let was_removed = self.indexes_unique.remove_index(name_interned);
        self.has_indexes_unique
            .store(self.indexes_unique.is_enabled(), Ordering::Release);

        if was_removed {
            self.save_index_info_unique().await?;
        }

        Ok(was_removed)
    }

    /// Сохраняет метаданные уникальных индексов в служебное хранилище.
    async fn save_index_info_unique(&self) -> DbResult<()> {
        let indexes_key = MetaKey::LegacyIndexesUnique.as_record_id().to_bytes();
        let bytes = bincode::serialize(&*self.indexes_unique)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store.set(indexes_key, Bytes::from(bytes)).await?;
        Ok(())
    }

    /// Ищет запись по значению уникального индекса.
    ///
    /// # Возвращает
    ///
    /// - `Ok(Some(RecordId))` — найдена одна запись
    /// - `Ok(None)` — запись не найдена
    /// - `Err` — ошибка чтения
    pub async fn lookup_by_unique_index(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Option<RecordId>> {
        self.check_unique_constraint(name_interned, values).await
    }

    /// Iterate over all unique index definitions.
    pub fn iter_unique_indexes(&self) -> impl Iterator<Item = IndexDefinition> + '_ {
        self.indexes_unique.iter()
    }

    /// Проверяет существование уникального индекса по его имени.
    pub fn unique_index_exists(&self, name_interned: u64) -> bool {
        self.indexes_unique.contains(name_interned)
    }

    /// Возвращает определение уникального индекса по его имени.
    pub fn get_unique_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes_unique.get_index(name_interned)
    }
}
