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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Менеджер индексов для одной таблицы.
///
/// Инкапсулирует всю логику работы с индексами:
/// - Хранит метаданные индексов в памяти
/// - Синхронизирует изменения с диском
/// - Обновляет индексы при изменении данных
pub struct TableIndexManager {
    /// Хранилище данных таблицы (для чтения записей при построении индекса)
    data_store: Arc<dyn Store>,
    /// Служебное хранилище для метаданных и записей индексов
    info_store: Arc<dyn Store>,

    /// Метаданные обычных индексов (неуникальных)
    indexes: Arc<RwLock<IndexInfo>>,
    /// Метаданные уникальных индексов
    indexes_unique: Arc<RwLock<IndexInfo>>,

    /// Атомарный флаг: есть ли хоть один обычный индекс
    /// Используется для быстрой проверки без захвата RwLock
    has_indexes: AtomicBool,
    /// Атомарный флаг: есть ли хоть один уникальный индекс
    has_indexes_unique: AtomicBool,
}

impl Clone for TableIndexManager {
    /// Создаёт клон менеджера индексов.
    ///
    /// Клонируются только Arc-ссылки на хранилища и данные индексов,
    /// а также копируются значения атомарных флагов.
    /// Это позволяет нескольким потокам работать с одним менеджером.
    fn clone(&self) -> Self {
        Self {
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            indexes_unique: Arc::clone(&self.indexes_unique),
            has_indexes: AtomicBool::new(self.has_indexes.load(Ordering::Relaxed)),
            has_indexes_unique: AtomicBool::new(self.has_indexes_unique.load(Ordering::Relaxed)),
        }
    }
}

impl TableIndexManager {
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

        // Инициализируем атомарные флаги на основе загруженных данных
        let has_indexes = AtomicBool::new(indexes.is_enabled());
        let has_indexes_unique = AtomicBool::new(indexes_unique.is_enabled());

        Ok(Self {
            data_store,
            info_store,
            indexes: Arc::new(RwLock::new(indexes)),
            indexes_unique: Arc::new(RwLock::new(indexes_unique)),
            has_indexes,
            has_indexes_unique,
        })
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
    /// - Идентификатора индекса (интернированное имя)
    /// - Хешей значений проиндексированных полей
    ///
    /// Это позволяет быстро находить записи по значению индексируемых полей.
    fn build_index_key(name_interned: u64, values: &[InnerValue]) -> IndexRecordKey {
        let value_refs: Vec<&InnerValue> = values.iter().collect();
        IndexRecordKey::new(false, name_interned).with_values(&value_refs)
    }

    /// Добавляет запись в индекс.
    ///
    /// Создаёт ключ вида: `[index_key][record_id]` и сохраняет пустое значение.
    /// Это позволяет:
    /// - Сканировать индекс по префиксу (все записи с данным значением)
    /// - Получить record_id из хвоста ключа
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
        // Строим ключ индекса (хеш значений)
        let index_key = Self::build_index_key(name_interned, values).to_bytes();

        // Добавляем record_id в конец ключа для уникальности
        // и возможности восстановления связи с записью
        let mut key = index_key.to_vec();
        key.extend_from_slice(&record_id.to_bytes());

        // Сохраняем с пустым значением (ключ сам содержит всю информацию)
        self.info_store.set(Bytes::from(key), Bytes::new()).await?;
        Ok(())
    }

    /// Удаляет запись из индекса.
    ///
    /// Удаляет ключ вида `[index_key][record_id]` из служебного хранилища.
    async fn remove_index_entry(
        &self,
        name_interned: u64,
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> DbResult<()> {
        let index_key = Self::build_index_key(name_interned, values).to_bytes();
        let mut key = index_key.to_vec();
        key.extend_from_slice(&record_id.to_bytes());
        self.info_store.remove(Bytes::from(key)).await?;
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
        {
            let indexes = self.indexes.write().await;
            indexes.add_index(index_def);
            self.has_indexes.store(true, Ordering::Release);
        }

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
        {
            let indexes = self.indexes.read().await;
            if !indexes.contains(name_interned) {
                return Ok(false);
            }
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
        let removed = {
            let indexes = self.indexes.write().await;
            let was_removed = indexes.remove_index(name_interned);
            // Обновляем флаг наличия индексов
            self.has_indexes
                .store(indexes.is_enabled(), Ordering::Release);
            was_removed
        };

        if removed {
            self.save_index_info().await?;
        }

        Ok(removed)
    }

    /// Сохраняет метаданные индексов в служебное хранилище.
    ///
    /// Сериализует IndexInfo через bincode и сохраняет под системным ключом.
    async fn save_index_info(&self) -> DbResult<()> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes = self.indexes.read().await.clone();
        let bytes =
            bincode::serialize(&indexes).map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
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

        let indexes = self.indexes.read().await;

        // Для каждого индекса извлекаем значения и добавляем запись
        for def in indexes.iter() {
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

        let indexes = self.indexes.read().await;

        for def in indexes.iter() {
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

        let indexes = self.indexes.read().await;

        // Для каждого индекса удаляем запись
        for def in indexes.iter() {
            if let Some(values) = Self::extract_index_values(old_value, &def.paths) {
                self.remove_index_entry(def.name_interned, &values, record_id)
                    .await?;
            }
        }

        Ok(())
    }
}
