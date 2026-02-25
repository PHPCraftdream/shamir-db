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

use crate::core::interner::InternerKey;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_record_key::IndexRecordKey;
use crate::db::storage::types::Store;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use bytes::Bytes;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    ) -> Result<Self, crate::db::DbError> {
        // Ключи для хранения метаданных индексов в служебном хранилище
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();

        // Загружаем обычные индексы или создаём пустую структуру
        let indexes = match info_store.get(indexes_key.clone()).await {
            Ok(bytes) => {
                // Десериализуем метаданные; при ошибке начинаем с пустого набора
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
            }
            Err(crate::db::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        // Загружаем уникальные индексы или создаём пустую структуру
        let indexes_unique = match info_store.get(indexes_unique_key.clone()).await {
            Ok(bytes) => {
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
            }
            Err(crate::db::DbError::NotFound(_)) => IndexInfo::new(),
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
        // Пустой путь означает "вернуть само значение"
        if path.is_empty() {
            return Some(value.clone());
        }

        match value {
            InnerValue::Map(map) => {
                // Получаем ключ для текущего уровня вложенности
                let key = InternerKey::new(path[0]);
                let next_value = map.get(&key)?;

                // Если это последний ключ в пути — возвращаем значение
                // Иначе рекурсивно углубляемся
                if path.len() == 1 {
                    Some(next_value.clone())
                } else {
                    Self::extract_value_by_path(next_value, &path[1..])
                }
            }
            // Если значение не Map, а путь не пуст — навигация невозможна
            _ => None,
        }
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
    fn extract_index_values(
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

    /// Добавляет запись в индекс.
    ///
    /// Ключ индекса: `[index_key]` (25 байт)
    /// Значение: сериализованный `BTreeSet<RecordId>` (множество идентификаторов записей)
    ///
    /// Если запись с таким index_key уже существует, добавляет record_id в множество.
    /// Иначе создаёт новое множество с одним record_id.
    ///
    /// # Аргументы
    ///
    /// * `name_interned` — интернированный идентификатор имени индекса
    /// * `values` — значения проиндексированных полей
    /// * `record_id` — идентификатор записи в таблице
    async fn add_index_entry(
        &self,
        name_interned: u64,
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> DbResult<()> {
        let index_key = Self::build_index_key(false, name_interned, values).to_bytes();

        // Читаем существующее множество RecordId или создаём пустое
        let mut record_ids = match self.info_store.get(index_key.clone()).await {
            Ok(bytes) => bincode::deserialize::<BTreeSet<RecordId>>(&bytes).unwrap_or_default(),
            Err(crate::db::DbError::NotFound(_)) => BTreeSet::new(),
            Err(e) => return Err(e),
        };

        // Добавляем новый RecordId (BTreeSet автоматически гарантирует уникальность)
        record_ids.insert(*record_id);

        // Сериализуем и сохраняем
        let bytes = bincode::serialize(&record_ids)
            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
        self.info_store.set(index_key, Bytes::from(bytes)).await?;
        Ok(())
    }

    /// Удаляет запись из индекса.
    ///
    /// Читает множество RecordId из значения индекса, удаляет указанный record_id
    /// и сохраняет обратно. Если множество становится пустым — удаляет ключ целиком.
    async fn remove_index_entry(
        &self,
        name_interned: u64,
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> DbResult<()> {
        use std::collections::BTreeSet;

        let index_key = Self::build_index_key(false, name_interned, values).to_bytes();

        // Читаем существующее множество RecordId
        let mut record_ids = match self.info_store.get(index_key.clone()).await {
            Ok(bytes) => bincode::deserialize::<BTreeSet<RecordId>>(&bytes).unwrap_or_default(),
            Err(crate::db::DbError::NotFound(_)) => return Ok(()), // Нечего удалять
            Err(e) => return Err(e),
        };

        // Удаляем RecordId из множества
        record_ids.remove(record_id);

        if record_ids.is_empty() {
            // Если множество пусто — удаляем ключ целиком
            self.info_store.remove(index_key).await?;
        } else {
            // Иначе сохраняем обновлённое множество
            let bytes = bincode::serialize(&record_ids)
                .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
            self.info_store.set(index_key, Bytes::from(bytes)).await?;
        }

        Ok(())
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

        let name_interned = index_def.name_interned;

        // Читаем данные таблицы потоком
        let mut stream = self.data_store.iter_stream(1000);

        let mut count = 0usize;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;

            for (key_bytes, value_bytes) in batch {
                // Пропускаем ключи, которые не являются RecordId (16 байт)
                let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                    Ok(a) => a,
                    Err(_) => continue,
                };
                let record_id = RecordId(arr);

                // Десериализуем значение записи
                let value = match InnerValue::from_bytes(value_bytes) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Если все поля индекса присутствуют — добавляем запись в индекс
                if let Some(values) = Self::extract_index_values(&value, &index_def.paths) {
                    self.add_index_entry(name_interned, &values, &record_id)
                        .await?;
                    count += 1;
                }
            }
        }

        // Добавляем определение индекса в метаданные и сохраняем
        self.indexes.add_index(index_def);
        self.has_indexes.store(true, Ordering::Release);
        self.save_index_info().await?;

        log::info!("Created index '{}' with {} entries", name_interned, count);
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

        // Формируем префикс для сканирования всех записей данного индекса
        let prefix = IndexRecordKey::new(false, name_interned).to_prefix_bytes();

        // Потоковое удаление записей индекса (избегаем загрузки в память)
        use futures::StreamExt;
        let mut stream = self.info_store.scan_prefix_stream(prefix, 1000);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, _) in batch {
                self.info_store.remove(key).await?;
            }
        }

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
        let indexes_key = RecordId::system("indexes").to_bytes();
        let bytes = bincode::serialize(&*self.indexes)
            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
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
        // Быстрая проверка — если индексов нет, ничего не делаем
        if !self.has_indexes() {
            return Ok(());
        }

        // Для каждого индекса извлекаем значения и добавляем запись
        for def in self.indexes.iter() {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                self.add_index_entry(def.name_interned, &values, record_id)
                    .await?;
            }
        }

        Ok(())
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
        if !self.has_indexes() {
            return Ok(());
        }

        for def in self.indexes.iter() {
            // Извлекаем старые и новые значения для данного индекса
            let old_values = Self::extract_index_values(old_value, &def.paths);
            let new_values = Self::extract_index_values(new_value, &def.paths);

            // Обрабатываем все варианты изменений
            match (old_values, new_values) {
                // Значения отсутствовали и отсутствуют — ничего не делаем
                (None, None) => {}
                // Появилось новое значение — добавляем в индекс
                (None, Some(new)) => {
                    self.add_index_entry(def.name_interned, &new, record_id)
                        .await?;
                }
                // Значение исчезло — удаляем из индекса
                (Some(old), None) => {
                    self.remove_index_entry(def.name_interned, &old, record_id)
                        .await?;
                }
                // Значение изменилось — обновляем индекс
                (Some(old), Some(new)) => {
                    if old != new {
                        self.remove_index_entry(def.name_interned, &old, record_id)
                            .await?;
                        self.add_index_entry(def.name_interned, &new, record_id)
                            .await?;
                    }
                    // Если old == new — ничего не делаем (оптимизация)
                }
            }
        }

        Ok(())
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
        if !self.has_indexes() {
            return Ok(());
        }

        // Для каждого индекса удаляем запись
        for def in self.indexes.iter() {
            if let Some(values) = Self::extract_index_values(old_value, &def.paths) {
                self.remove_index_entry(def.name_interned, &values, record_id)
                    .await?;
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
        let index_key = Self::build_index_key(false, name_interned, values).to_bytes();

        match self.info_store.get(index_key).await {
            Ok(bytes) => {
                let record_ids =
                    bincode::deserialize::<BTreeSet<RecordId>>(&bytes).unwrap_or_default();
                Ok(record_ids)
            }
            Err(crate::db::DbError::NotFound(_)) => Ok(BTreeSet::new()),
            Err(e) => Err(e),
        }
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

        for def in self.indexes_unique.iter() {
            if let Some(values) = Self::extract_index_values(value, &def.paths) {
                if let Some(existing_id) = self
                    .check_unique_constraint(def.name_interned, &values)
                    .await?
                {
                    return Err(crate::db::DbError::DuplicateKey(format!(
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

        for def in self.indexes_unique.iter() {
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
                        return Err(crate::db::DbError::DuplicateKey(format!(
                            "Unique index '{}' violated: value already exists for record {:?}",
                            def.name_interned, existing_id
                        )));
                    }
                }
            }
        }

        Ok(())
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
            Err(crate::db::DbError::NotFound(_)) => Ok(None),
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

        for def in self.indexes_unique.iter() {
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

        for def in self.indexes_unique.iter() {
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

        for def in self.indexes_unique.iter() {
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
        use std::collections::HashMap;

        let name_interned = index_def.name_interned;
        // HashMap для отслеживания количества каждого значения
        let mut value_counts: HashMap<Vec<u8>, usize> = HashMap::new();
        // Записи для добавления в индекс (RecordId, values_key)
        let mut entries: Vec<(RecordId, Vec<u8>, Vec<InnerValue>)> = Vec::new();

        // Первый проход: подсчёт значений
        {
            use futures::StreamExt;
            let mut stream = self.data_store.iter_stream(1000);

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

                    if let Some(values) = Self::extract_index_values(&value, &index_def.paths) {
                        let values_key = bincode::serialize(&values)
                            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;

                        *value_counts.entry(values_key.clone()).or_insert(0) += 1;
                        entries.push((record_id, values_key, values));
                    }
                }
            }
        }

        // Проверяем наличие дубликатов
        let duplicates: Vec<(&Vec<u8>, &usize)> =
            value_counts.iter().filter(|(_, &c)| c > 1).collect();

        if !duplicates.is_empty() {
            // Считаем общее количество записей с дублирующимися значениями
            let duplicate_record_count: usize = duplicates.iter().map(|(_, &c)| c).sum();

            // Получаем пример дублирующегося значения для сообщения об ошибке
            let sample_key = duplicates[0].0;
            let sample_values: Vec<InnerValue> =
                bincode::deserialize(sample_key).unwrap_or_else(|_| vec![InnerValue::Null]);

            // Форматируем пример значения
            let sample_str = Self::format_values_for_error(&sample_values);

            return Err(crate::db::DbError::UniqueIndexCreationFailed(
                name_interned.to_string(),
                duplicate_record_count,
                sample_str,
            ));
        }

        // Дубликатов нет — добавляем записи в индекс
        let mut count = 0usize;
        for (record_id, _values_key, values) in entries {
            self.add_unique_entry(name_interned, &values, &record_id)
                .await?;
            count += 1;
        }

        // Добавляем определение индекса в метаданные и сохраняем
        self.indexes_unique.add_index(index_def);
        self.has_indexes_unique.store(true, Ordering::Release);
        self.save_index_info_unique().await?;

        log::info!(
            "Created unique index '{}' with {} entries",
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

        // Формируем префикс для сканирования всех записей данного индекса
        let prefix = IndexRecordKey::new(true, name_interned).to_prefix_bytes();

        // Потоковое удаление записей индекса
        use futures::StreamExt;
        let mut stream = self.info_store.scan_prefix_stream(prefix, 1000);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, _) in batch {
                self.info_store.remove(key).await?;
            }
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
        let indexes_key = RecordId::system("indexes_unique").to_bytes();
        let bytes = bincode::serialize(&*self.indexes_unique)
            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
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

    /// Проверяет существование уникального индекса по его имени.
    pub fn unique_index_exists(&self, name_interned: u64) -> bool {
        self.indexes_unique.contains(name_interned)
    }

    /// Возвращает определение уникального индекса по его имени.
    pub fn get_unique_index_definition(&self, name_interned: u64) -> Option<IndexDefinition> {
        self.indexes_unique.get_index(name_interned)
    }
}
