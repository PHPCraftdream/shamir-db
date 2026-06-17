//! Уникальные индексы — все методы `*_unique*` менеджера индексов.
//!
//! Реализация вынесена в отдельный файл для разделения ответственности:
//! этот модуль отвечает за гарантии уникальности значений.

use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_keys::{build_index_key, build_index_key_from_record};
use crate::legacy::index_manager::IndexManager;
use crate::legacy::index_record_key::IndexRecordKey;
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::Ordering;

impl IndexManager {
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
    pub async fn validate_unique_for_create(
        &self,
        value: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                let index_key = irk.to_bytes();
                if let Some(existing_id) = self.check_unique_key(&index_key).await? {
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
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            let old_key =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(true, def.name_interned, new_value, &def.paths);

            // If the key is unchanged or both absent, skip.
            match (&old_key, &new_key) {
                (None, None) => continue,
                (Some(o), Some(n)) if o.to_bytes() == n.to_bytes() => continue,
                _ => {}
            }

            // Check the new key (if present).
            if let Some(nk) = &new_key {
                let index_key = nk.to_bytes();
                if let Some(existing_id) = self.check_unique_key(&index_key).await? {
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
    /// returns the key that `check_unique_constraint`
    /// (and `add_unique_entry`) read/write. The tx commit path records
    /// these as `UniqueGuard`s and re-validates them under `commit_lock`,
    /// closing the tx-concurrent unique hole.
    ///
    /// Returns an empty vec when there are no unique indexes or the
    /// value populates none of them.
    pub fn unique_keys_for(&self, value: &(impl RecordRef + ?Sized)) -> Vec<Bytes> {
        if !self.has_unique_indexes() {
            return Vec::new();
        }
        let mut keys = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                keys.push(irk.to_bytes());
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
    pub(super) async fn check_unique_constraint(
        &self,
        name_interned: u64,
        values: &[InnerValue],
    ) -> DbResult<Option<RecordId>> {
        let index_key = build_index_key(true, name_interned, values).to_bytes();
        self.check_unique_key(&index_key).await
    }

    /// Check a unique constraint by its pre-computed index key bytes.
    async fn check_unique_key(&self, index_key: &Bytes) -> DbResult<Option<RecordId>> {
        match self.info_store.get(index_key.clone()).await {
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

    /// Добавляет запись в уникальный индекс by pre-computed key.
    ///
    /// Ключ: `[index_key with is_unique=true]` (25 байт)
    /// Значение: `RecordId` (16 байт)
    ///
    /// # Важно
    ///
    /// Не проверяет уникальность! Вызывай `validate_unique_*` перед этим методом.
    async fn add_unique_entry_by_key(
        &self,
        index_key: Bytes,
        record_id: &RecordId,
    ) -> DbResult<()> {
        self.info_store.set(index_key, record_id.to_bytes()).await?;
        Ok(())
    }

    /// Удаляет запись из уникального индекса by pre-computed key.
    async fn remove_unique_entry_by_key(&self, index_key: Bytes) -> DbResult<()> {
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
        value: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                self.add_unique_entry_by_key(irk.to_bytes(), record_id)
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
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            let old_key =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(true, def.name_interned, new_value, &def.paths);

            match (old_key, new_key) {
                (None, None) => {}
                (None, Some(nk)) => {
                    self.add_unique_entry_by_key(nk.to_bytes(), record_id)
                        .await?;
                }
                (Some(ok), None) => {
                    self.remove_unique_entry_by_key(ok.to_bytes()).await?;
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        self.remove_unique_entry_by_key(old_bytes).await?;
                        self.add_unique_entry_by_key(new_bytes, record_id).await?;
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
        old_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in defs {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths)
            {
                self.remove_unique_entry_by_key(irk.to_bytes()).await?;
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
        // Collect (record_id, index_key_bytes) for duplicate detection and bulk write.
        let mut key_counts: TMap<Bytes, usize> = new_map();
        let mut entries: Vec<(RecordId, Bytes)> = Vec::new();

        for (record_id, value) in &records {
            if let Some(irk) =
                build_index_key_from_record(true, name_interned, value, &index_def.paths)
            {
                let index_key = irk.to_bytes();
                *key_counts.entry(index_key.clone()).or_insert(0) += 1;
                entries.push((*record_id, index_key));
            }
        }

        let duplicates: Vec<(&Bytes, &usize)> =
            key_counts.iter().filter(|(_, &c)| c > 1).collect();
        if !duplicates.is_empty() {
            let duplicate_record_count: usize = duplicates.iter().map(|(_, &c)| c).sum();
            // Sample: we can't decode the hash back to values, so give a generic message.
            let sample_str = "<duplicate indexed values>".to_string();
            return Err(shamir_storage::error::DbError::UniqueIndexCreationFailed(
                name_interned.to_string(),
                duplicate_record_count,
                sample_str,
            ));
        }

        let count = entries.len();
        let mut writes: Vec<(Bytes, Bytes)> = Vec::with_capacity(count);
        for (record_id, index_key) in entries {
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
    pub(super) async fn save_index_info_unique(&self) -> DbResult<()> {
        let indexes_key = RecordId::system("indexes_unique").to_bytes();
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

    // ============================================================================
    // UNIQUE INDEXES - Planner variants
    // ============================================================================

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
        value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                ops.push(IndexWriteOp::SetPosting {
                    key: irk.to_bytes(),
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
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            let old_key =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(true, def.name_interned, new_value, &def.paths);
            match (old_key, new_key) {
                (None, None) => {}
                (None, Some(nk)) => {
                    ops.push(IndexWriteOp::SetPosting {
                        key: nk.to_bytes(),
                        value: Bytes::copy_from_slice(record_id.as_bytes()),
                    });
                }
                (Some(ok), None) => {
                    ops.push(IndexWriteOp::RemovePosting { key: ok.to_bytes() });
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        ops.push(IndexWriteOp::RemovePosting { key: old_bytes });
                        ops.push(IndexWriteOp::SetPosting {
                            key: new_bytes,
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
        old_value: &(impl RecordRef + ?Sized),
    ) -> DbResult<Vec<IndexWriteOp>> {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths)
            {
                ops.push(IndexWriteOp::RemovePosting { key: irk.to_bytes() });
            }
        }
        Ok(ops)
    }

    /// Planner variant of `on_records_created_unique_batch` — returns
    /// `Vec<IndexWriteOp>`. Uniqueness validation (collision detection)
    /// stays in the plan phase: it reads existing postings to detect
    /// duplicates. If collision → `Err(DuplicateKey(...))`.
    pub async fn plan_records_created_unique_batch<'a, R, I>(
        &self,
        items: I,
    ) -> DbResult<Vec<IndexWriteOp>>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        if !self.has_unique_indexes() {
            return Ok(Vec::new());
        }
        let mut ops = Vec::new();
        for def in self.indexes_unique.iter() {
            for (rid, value) in items.clone() {
                if let Some(irk) =
                    build_index_key_from_record(true, def.name_interned, value, &def.paths)
                {
                    ops.push(IndexWriteOp::SetPosting {
                        key: irk.to_bytes(),
                        value: Bytes::copy_from_slice(rid.as_bytes()),
                    });
                }
            }
        }
        Ok(ops)
    }

    /// Batched version of `on_record_created_unique`. Same borrow
    /// shape as `on_records_created_batch`.
    pub async fn on_records_created_unique_batch<'a, R, I>(&self, items: I) -> DbResult<()>
    where
        R: RecordRef + ?Sized + 'a,
        I: IntoIterator<Item = (&'a RecordId, &'a R)> + Clone,
    {
        let ops = self.plan_records_created_unique_batch(items).await?;
        self.apply_ops(&ops).await
    }
}
