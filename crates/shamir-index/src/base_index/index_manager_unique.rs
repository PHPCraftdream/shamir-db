//! Уникальные индексы — все методы `*_unique*` менеджера индексов.
//!
//! Реализация вынесена в отдельный файл для разделения ответственности:
//! этот модуль отвечает за гарантии уникальности значений.

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::{build_index_key, build_index_key_from_record};
use crate::base_index::index_manager::IndexManager;
use crate::base_index::write_barrier_flags::UNIQUE_INDEX_EXISTS;
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_tx::{IndexFamily, Provenance};
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// R0-B (#1008): [`Provenance`] for an op planned against `def` on the
/// UNIQUE base_index family. See `IndexDefinition::provenance`'s doc.
fn unique_provenance(def: &IndexDefinition) -> Provenance {
    def.provenance(IndexFamily::Unique)
}

// ── #1098 round-2 test-only pause seam ──────────────────────────────────
//
// Mirrors `shamir-engine`'s `table_manager_tx_ops::PostGenCapturePreUniqueCheckHook`
// pattern (itself mirroring `pre_commit.rs`'s `PostPrelockPreMaterializeHook`):
// a `#[cfg(test)]` `OnceLock<Arc<Hook>>` global, zero cost when unset. Fires
// in `create_unique_index_from_records` strictly AFTER the flag is set and
// BEFORE the generation is bumped — the specific ordering #1098 round 2's
// writer-side fix establishes. A test can park a CREATE here, drive a
// concurrent tx's `insert_tx` to completion while parked (its generation
// capture sees the OLD, pre-bump value; its LATER `has_unique_indexes()`
// read sees the ALREADY-set flag — both safe, by design), then resume —
// proving the writer's flag-then-gen order is what makes the reader's
// gen-then-checks order actually sufficient.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PostFlagSetPreGenBumpHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
pub(crate) static TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostFlagSetPreGenBumpHook>,
> = std::sync::OnceLock::new();

/// Parks on [`TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK`] if a test installed
/// one; a true no-op otherwise. One-shot (CAS true→false) so only the
/// FIRST caller to reach this seam actually parks.
async fn fire_post_flag_set_pre_gen_bump_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

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
        self.validate_unique_for_create_with_defs(value, &defs)
            .await
    }

    /// Variant of [`validate_unique_for_create`] that accepts pre-collected
    /// unique-index definitions, avoiding a per-call DashMap iteration when the
    /// caller already has a batch-scope snapshot.
    ///
    /// Use this from batch insert paths where definitions are stable for the
    /// duration of the batch. Standalone callers should keep using
    /// [`validate_unique_for_create`].
    pub async fn validate_unique_for_create_with_defs(
        &self,
        value: &(impl RecordRef + ?Sized),
        defs: &[IndexDefinition],
    ) -> DbResult<()> {
        if defs.is_empty() {
            return Ok(());
        }
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

    /// Like `validate_unique_for_create`, but a durable conflict is NOT an
    /// error if the conflicting index_key is present in `released_in_tx`
    /// **AND** the durable owner currently holding that key
    /// (`existing_id`) is itself a record this SAME transaction has staged
    /// a write (Set or Remove) for, per `touched_records_in_tx` — the
    /// caller (`insert_tx`) has already determined, by walking its own
    /// `tx.index_write_set`, that this key was released by a `RemovePosting`
    /// somewhere in this tx, and by consulting `tx.write_set`, that the
    /// durable owner is a record this tx genuinely mutated.
    ///
    /// #1096 follow-up (found by `@oh` review): checking `released_in_tx`
    /// alone is NOT sufficient and is a genuine uniqueness-bypass hazard.
    /// Under `Snapshot` isolation (last-writer-wins, no write-write
    /// conflict detection — see `pre_commit.rs`'s `claim_write_set`), this
    /// tx's own plan to release `index_key` was built against a
    /// possibly-STALE snapshot: by the time this check runs, a DIFFERENT,
    /// concurrently-committed tx may have already reclaimed the same key
    /// for an unrelated record. `released_in_tx.contains(index_key)` alone
    /// cannot distinguish "the record I'm about to release still durably
    /// owns this key" from "someone else already claimed it after my
    /// snapshot was taken" — both look identical from `index_key` alone.
    /// Cross-checking that the CURRENT durable owner (`existing_id`) is a
    /// record THIS tx has touched closes that hole: if a concurrent tx won
    /// the race, `existing_id` is a record this tx never staged a write
    /// for, so `touched_records_in_tx` correctly rejects the tolerance and
    /// this call falls through to `DuplicateKey`.
    pub async fn validate_unique_for_create_with_released(
        &self,
        value: &(impl RecordRef + ?Sized),
        released_in_tx: &shamir_collections::TFxSet<Vec<u8>>,
        touched_records_in_tx: &shamir_collections::TFxSet<[u8; 16]>,
    ) -> DbResult<()> {
        if !self.has_unique_indexes() {
            return Ok(());
        }
        let defs: Vec<IndexDefinition> = self.indexes_unique.iter().collect();
        for def in &defs {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                let index_key = irk.to_bytes();
                if let Some(existing_id) = self.check_unique_key(&index_key).await? {
                    if released_in_tx.contains(index_key.as_ref())
                        && touched_records_in_tx.contains(existing_id.as_bytes())
                    {
                        continue; // released earlier in this same tx — safe to reclaim
                    }
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

    /// Like `validate_unique_for_update`, but a durable conflict on the
    /// NEW key is NOT an error under the same tx-aware release-then-reclaim
    /// tolerance as [`validate_unique_for_create_with_released`] — see that
    /// method's doc for the full rationale (including why checking
    /// `released_in_tx` alone would be a uniqueness-bypass hazard under
    /// `Snapshot` isolation's stale-read possibility).
    ///
    /// #1096 follow-up: an UPDATE can also be the reclaiming half of a
    /// release-then-reclaim pair (`tx: DELETE A{email:"x"}; UPDATE C SET
    /// email="x"`), not just an INSERT — this method closes that call site.
    pub async fn validate_unique_for_update_with_released(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
        released_in_tx: &shamir_collections::TFxSet<Vec<u8>>,
        touched_records_in_tx: &shamir_collections::TFxSet<[u8; 16]>,
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
                    if &existing_id == record_id {
                        continue;
                    }
                    if released_in_tx.contains(index_key.as_ref())
                        && touched_records_in_tx.contains(existing_id.as_bytes())
                    {
                        continue; // released earlier in this same tx — safe to reclaim
                    }
                    return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                        "Unique index '{}' violated: value already exists for record {:?}",
                        def.name_interned, existing_id
                    )));
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
        let mut keys = Vec::with_capacity(4);
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
    ///
    /// P0-3a (#1011) — why the UNIQUE family gets NO `ReaderDrainGate`
    /// (unlike the regular family's `lookup_by_index`): this is the unique
    /// family's only production read chokepoint, and its ONLY production
    /// callers are `validate_unique_for_create` / `validate_unique_for_update`
    /// (via `check_unique_constraint`), BOTH of which are already serialized
    /// against DROP UNIQUE by the per-table `unique_write_lock`. The writer
    /// takes `unique_write_lock` BEFORE calling validate
    /// (`table_manager_crud.rs`: the `unique_write_lock.lock().await` at the
    /// insert/update paths precedes `validate_unique_for_create`/`_update`),
    /// and DROP UNIQUE (`drop_unique_index` → `TableManager::drop_unique_index`)
    /// runs under the same `begin_write_barrier` admission serialization. The
    /// commit-time equivalent is covered too: `pre_commit.rs` Phase 2.5 acquires
    /// every relevant table's `unique_write_lock` and holds it across Phase 2.6
    /// (which reads `info_store().get()` directly under that lock). So a DROP
    /// UNIQUE cannot physically overlap a `check_unique_key` read — adding a
    /// gate here would serialize an already-serialized path.
    ///
    /// TRIPWIRE: `lookup_by_unique_index` (which delegates here) is TEST-ONLY
    /// today (verified by workspace grep — every caller lives under a `tests/`
    /// dir). If it ever gains a PRODUCTION caller that is NOT already inside a
    /// `unique_write_lock` critical section, that caller's read path MUST get
    /// its own `ReaderDrainGate` acquisition at that point — do NOT silently
    /// rely on the unique-family lock serialization holding for a new caller.
    async fn check_unique_key(&self, index_key: &Bytes) -> DbResult<Option<RecordId>> {
        match self.info_store.get(index_key.clone().into()).await {
            Ok(bytes) => {
                // P0-4 (#960): a unique-index posting MUST store exactly a
                // 16-byte `RecordId`. Any other length is NECESSARILY genuine
                // corruption of the posting value. Fail CLOSED by surfacing a
                // typed `Codec` error, mirroring the F-83 (#911) corruption
                // policy for `system:indexes` / `system:indexes_unique` blobs
                // in `IndexManager::new`.
                //
                // The prior code logged a warning and returned `Ok(None)` —
                // the SAME value as the genuine not-found arm below, which
                // callers (`validate_unique_for_create/update`,
                // `lookup_by_unique_index`) treat as "key is free". A
                // corrupted unique posting was therefore silently treated as
                // an EMPTY, insertable key, the opposite of this codebase's
                // fail-closed policy: a subsequent write could pass
                // unique-constraint validation and commit a duplicate on top
                // of corrupted storage.
                //
                // The `NotFound` arm below remains the ONLY legitimate "key is
                // free" path (a key that was never written). The `try_into`
                // maps any length other than 16 (including 0, 15, 17, ...) to
                // the typed error, so there is no infallible `unwrap()` left
                // that would silently break if this path is ever reworked.
                let arr: [u8; 16] = bytes.as_ref().try_into().map_err(|_| {
                    shamir_storage::error::DbError::Codec(format!(
                        "unique index posting corrupt: expected 16-byte RecordId, \
                         got {} bytes (genuine corruption — fail-closed)",
                        bytes.len()
                    ))
                })?;
                Ok(Some(RecordId(arr)))
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
        self.info_store
            .set(index_key.into(), record_id.to_bytes())
            .await?;
        Ok(())
    }

    /// Удаляет запись из уникального индекса by pre-computed key.
    async fn remove_unique_entry_by_key(&self, index_key: Bytes) -> DbResult<()> {
        self.info_store.remove(index_key.into()).await?;
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
        //
        // P2 (#1023): a malformed key (not exactly 16 bytes) or an
        // undecodable value now ABORTS the backfill with a typed
        // `DbError::Codec`, instead of silently `continue`-ing past the
        // row. Fail-open here previously left the row's unique posting
        // never written — its "occupied" state became invisible to later
        // duplicate-detection, so a subsequent insert with a colliding
        // value could be wrongly accepted on top of the gap. Mirrors the
        // fail-closed policy `#960` already established for a corrupt
        // EXISTING unique posting a few methods above
        // (`check_unique_key`'s `try_into` → `DbError::Codec`, not
        // `Ok(None)`): both are "genuine corruption" abort. No existing
        // caller or test relies on the lenient skip (verified: this is the
        // only call site for `create_unique_index`'s live data_store scan;
        // `create_unique_index_from_records`'s callers all pass
        // already-decoded records and never see this loop).
        let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
        let mut records: Vec<(RecordId, InnerValue)> = Vec::with_capacity(4);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key_bytes, value_bytes) in batch {
                let arr: [u8; 16] = key_bytes.as_ref().try_into().map_err(|_| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_unique_index backfill: malformed record key, expected \
                         16-byte RecordId, got {} bytes (genuine corruption — fail-closed, \
                         aborting backfill rather than silently skipping the row)",
                        key_bytes.as_ref().len()
                    ))
                })?;
                let record_id = RecordId(arr);
                let value = InnerValue::from_bytes(value_bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_unique_index backfill: record {record_id:?} value failed to \
                         decode: {e} (fail-closed, aborting backfill rather than silently \
                         skipping the row)"
                    ))
                })?;
                records.push((record_id, value));
            }
        }

        self.create_unique_index_from_records(index_def, records)
            .await
    }

    /// FINAL-A: create unique index and backfill from pre-decoded records
    /// instead of `data_store.iter_stream`. Used by `TableManager` when an
    /// MvccStore is attached.
    ///
    /// # F-78 (#905) — DEFERRED (regular-hash family landed; unique deferred)
    ///
    /// Unlike the regular-hash family (whose `create_index_from_records` got a
    /// streaming counterpart `create_index_from_stream` in F-78), this unique
    /// path STILL materializes: it builds a `TMap<Bytes, usize>` duplicate-
    /// count map PLUS an `entries: Vec<(RecordId, Bytes)>` — two O(table)
    /// structures — before any posting is written, then (if duplicate-free) a
    /// THIRD O(table) `Vec` for the final `set_many`. That is the O(table)
    /// peak-memory shape F-78 eliminates for regular indexes, preserved here
    /// UNCHANGED because the unique family cannot stream naively: duplicate
    /// detection needs GLOBAL knowledge (you cannot know a key is duplicate-
    /// free until you have seen every row with that key), and a sound bounded-
    /// memory rewrite (external/partitioned duplicate detection, or a temporary
    /// unique backend whose set-primitive rejects duplicates at write time) is
    /// a substantially harder, separately-scoped task. Per F-78's brief
    /// escape hatch, the regular-hash streaming fix was landed fully tested +
    /// benchmarked and this unique-family gap is documented as deferred
    /// follow-up rather than shipping an unsound or untested duplicate-
    /// detection rewrite under schedule pressure. The existing duplicate-
    /// detection + `UniqueIndexCreationFailed` error shape are unchanged.
    pub async fn create_unique_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        use shamir_types::types::common::{new_map, TMap};

        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight — mirrors `create_index`'s regular-family guard.
        if self
            .dropping_unique
            .lock()
            .unwrap()
            .contains_key(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create unique index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Collect (record_id, index_key_bytes) for duplicate detection and bulk write.
        let mut key_counts: TMap<Bytes, usize> = new_map();
        let mut entries: Vec<(RecordId, Bytes)> = Vec::with_capacity(16);

        for (record_id, value) in &records {
            if let Some(irk) =
                build_index_key_from_record(true, name_interned, value, &index_def.paths)
            {
                let index_key = irk.to_bytes();
                *key_counts.entry(index_key.clone()).or_insert(0) += 1;
                entries.push((*record_id, index_key));
            }
        }

        let duplicates: Vec<(&Bytes, &usize)> = key_counts.iter().filter(|(_, &c)| c > 1).collect();
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
        // `RecordKey` keys (fed to the store `set_many`); index keys are
        // built as `Bytes` and converted byte-identically at each push.
        let mut writes: Vec<(RecordKey, Bytes)> = Vec::with_capacity(count);
        for (record_id, index_key) in entries {
            writes.push((
                index_key.into(),
                Bytes::copy_from_slice(record_id.as_bytes()),
            ));
        }
        if !writes.is_empty() {
            self.info_store.set_many(writes).await?;
        }

        self.indexes_unique.add_index(index_def);
        // #1098 round-2 review: the flag MUST be set BEFORE the generation
        // is bumped, not after. `bump_generation`'s `fetch_add` is `AcqRel`
        // (`index_manager.rs`), so a reader that observes the NEW
        // generation via an `Acquire` load is guaranteed (by that
        // synchronizes-with edge, plus program order on this single
        // writer) to also observe every write this thread made BEFORE the
        // bump — including this flag set. With the flag set AFTER the
        // bump (the pre-#1098-round-2 order), a reader could observe the
        // new generation while the flag store hadn't landed yet, silently
        // reopening the exact race #1098's reader-side reorder (`gen`
        // captured before the `has_unique_indexes()` check) was meant to
        // close: the reader's `has_unique_indexes()` read could still see
        // `false` even though its OWN generation capture already reflects
        // this CREATE, defeating the commit-time `stage_gen != mgr.generation()`
        // rederive gate. F-69 (#896): still SeqCst, still the single
        // locked-instruction cost class the `write_barrier_flags.rs`
        // module doc describes — only the ORDER relative to
        // `bump_generation` changed, not the atomicity of the flag set
        // itself.
        self.write_barrier_flags.set(UNIQUE_INDEX_EXISTS);

        // #1098 round-2 test-only pause seam: fires strictly AFTER the flag
        // set above and BEFORE the generation bump below — see
        // `fire_post_flag_set_pre_gen_bump_test_hook`'s doc. No-op in every
        // non-test build.
        fire_post_flag_set_pre_gen_bump_test_hook().await;

        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive

        // P1-2 (#967): the posting entries are ALREADY durably written by the
        // `set_many` above. If THIS definition persist fails, the postings are
        // orphaned — on restart, no definition loads but postings remain.
        // NOTE: Cannot write DdlOpState::Failed here because this layer
        // (IndexManagerUnique) does not have op_id in scope.
        self.save_index_info_unique().await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE UNIQUE INDEX '{name_interned}': the index posting \
                 entries were durably written, but persisting the index \
                 definition failed: {e}. The index definition is NOT persisted — \
                 on restart, orphan postings will exist without a corresponding \
                 definition. Call TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            ))
        })?;

        log::info!(
            "Created unique index '{}' with {} entries (from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// Удаляет уникальный индекс по его имени.
    ///
    /// F-76 (#903) + P0-3 (#959): definition retired BEFORE the posting
    /// sweep (F-76, same mirror-image-of-F-72 fix as `drop_index`), with a
    /// durable tombstone (P0-3) closing the crash-resurrection gap (3c) and
    /// the name-reuse ghost-postings gap (3b). See `IndexManager::drop_index`'s
    /// doc for the full sub-bug 3a/3b/3c write-up — this method mirrors it
    /// exactly for the unique family (tombstone key:
    /// `system:indexes_unique_dropping`).
    ///
    /// # Возвращает
    ///
    /// `true` — индекс существовал и был удалён
    /// `false` — индекс не найден
    ///
    /// #1051: accepts `op_id` minted at dispatch time for crash recovery status writes.
    ///
    /// #1069: if `op_id` and `index_name` are both provided, writes terminal
    /// `Succeeded` status BEFORE clearing the tombstone (ensuring crash-safety for
    /// the inline path). The recovery paths write their own status, so this is
    /// only needed for the synchronous success path.
    pub async fn drop_unique_index(
        &self,
        name_interned: u64,
        op_id: Option<String>,
        index_name: Option<&str>,
    ) -> DbResult<bool> {
        if !self.indexes_unique.contains(name_interned) {
            return Ok(false);
        }

        // P0-3 (#959 / #1051): write a durable tombstone BEFORE retiring/sweeping.
        // See `drop_index`'s doc for the crash-state matrix.
        // Clone op_id so we still have it for status write later.
        let op_id_clone = op_id.clone();
        self.add_to_dropping(true, name_interned, op_id).await?;

        // F-76 (#903): retire the definition FIRST (RCU swap publishes a Vec
        // without this definition atomically; the shared write-barrier bit is
        // cleared so writers stop maintaining it).
        let was_removed = self.indexes_unique.remove_index(name_interned);
        // #1098 round-2 review: flag BEFORE generation bump, mirroring the
        // fix in `create_unique_index_from_records` — see that call site's
        // comment for the full happens-before argument. Here a wrong order
        // is lower severity (a reader that captures the new generation
        // before observing the cleared flag would over-validate against a
        // since-dropped index, a spurious `UniqueViolation`, not a silent
        // duplicate) but the invariant should stay uniform across both
        // publish sites. F-69 (#896): still SeqCst set/clear on the shared
        // packed word.
        self.write_barrier_flags
            .set_to(UNIQUE_INDEX_EXISTS, self.indexes_unique.is_enabled());
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive

        // F-76 test seam (shared with the regular-hash drop hook). Park here
        // (definition already retired, postings not yet swept) if a test
        // installed a pause hook. NOT `#[cfg(test)]`-gated — cross-crate test
        // consumer.
        if let Some(hook) = self.drop_index_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Sweep the (now orphan, planner-invisible) posting entries.
        // P0-3 (#959): extracted to `sweep_index_postings` for reuse by
        // the recovery path.
        // P1-2 (#967): a durable tombstone is already persisted — enrich
        // the error if this sweep fails.
        // NOTE: Cannot write DdlOpState::Failed here even though `op_id` is
        // in scope (a parameter of this function, #1051) — this crate
        // (`shamir-index`) has no access to `shamir-engine`'s `ddl_op_log`,
        // which sits one layer up.
        self.sweep_index_postings(true, name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP UNIQUE INDEX '{name_interned}': a durable drop tombstone \
                 was persisted and the definition was retired from the planner, \
                 but the posting sweep failed: {e}. On restart, recovery will \
                 resume the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                ))
            })?;

        // P0-3 (#959) test seam — post-sweep, pre-persist crash window.
        // See `drop_index`'s matching hook for the full rationale.
        if let Some(hook) = self.drop_index_post_sweep_hook.load_full() {
            hook.wait_at_window().await;
        }

        // Persist the reduced IndexInfo (definition removed).
        // P1-2 (#967): the tombstone is still in place — if this persist
        // fails, recovery will see the tombstone and finish the drop.
        // NOTE: Cannot write DdlOpState::Failed here even though `op_id` is
        // in scope (a parameter of this function, #1051) — this crate
        // (`shamir-index`) has no access to `shamir-engine`'s `ddl_op_log`,
        // which sits one layer up.
        if was_removed {
            self.save_index_info_unique().await.map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP UNIQUE INDEX '{name_interned}': a durable drop \
                     tombstone was persisted, the definition was retired, and \
                     the posting sweep completed, but persisting the reduced \
                     index metadata failed: {e}. On restart, recovery will \
                     finish the drop idempotently. Call TableManager::verify() \
                     to confirm state."
                ))
            })?;
        }

        // #1069: Write terminal Succeeded status BEFORE clearing tombstone.
        // This ensures crash-safety for the inline path.
        if was_removed {
            if let (Some(op_id_str), Some(index_name_str)) = (op_id_clone.as_deref(), index_name) {
                let op_id_parsed =
                    <RecordId as std::str::FromStr>::from_str(op_id_str).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!("Invalid op_id: {e}"))
                    })?;
                let status = shamir_query_types::read::DdlOpStatus {
                    op_id: op_id_parsed,
                    kind: shamir_query_types::read::DdlOpKind::DropUniqueHashIndex {
                        index_name: index_name_str.to_string(),
                    },
                    state: shamir_query_types::read::DdlOpState::Succeeded {
                        completed_at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64,
                    },
                };
                // Do not swallow status-write errors — log loudly.
                if let Err(e) = super::ddl_op_log::write_op_status(&self.info_store, &status).await
                {
                    log::error!(
                        "#1069: DROP UNIQUE INDEX '{}' succeeded, but failed to write \
                         Succeeded status: {}. The drop completed successfully, but polling by \
                         op_id {:?} will return Unknown. Call TableManager::verify() to confirm \
                         the index was dropped.",
                        index_name_str,
                        e,
                        op_id_parsed
                    );
                }
            }
        }

        // #1069 test seam (shared with the regular-hash drop hook). Park
        // here (terminal Succeeded status already durably written,
        // tombstone NOT yet cleared) if a test installed the status pause
        // hook. NOT `#[cfg(test)]`-gated — cross-crate test consumer.
        if let Some(hook) = self.drop_index_status_pause_hook.load_full() {
            hook.wait_at_window().await;
        }

        // P0-3 (#959): clear the tombstone AFTER the reduced IndexInfo is
        // durably persisted. See `drop_index`'s matching call for the
        // ordering rationale.
        // P1-2 (#967): if this fails, the tombstone remains — recovery
        // will just clear it (a no-op on the already-finished drop).
        self.clear_from_dropping(true, name_interned)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "DROP UNIQUE INDEX '{name_interned}': the drop is essentially \
                 complete (tombstone persisted, definition retired, sweep done, \
                 reduced metadata persisted), but clearing the drop tombstone \
                 failed: {e}. On restart, recovery will clear the tombstone as \
                 a no-op. Call TableManager::verify() to confirm state."
                ))
            })?;

        Ok(was_removed)
    }

    /// Сохраняет метаданные уникальных индексов в служебное хранилище.
    pub(super) async fn save_index_info_unique(&self) -> DbResult<()> {
        let indexes_key = RecordId::system("indexes_unique").to_bytes();
        let bytes = bincode::serialize(&*self.indexes_unique)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        self.info_store
            .set(indexes_key.into(), Bytes::from(bytes))
            .await?;
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
                    provenance: unique_provenance(&def),
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
            let provenance = unique_provenance(&def);
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
                        provenance,
                    });
                }
                (Some(ok), None) => {
                    ops.push(IndexWriteOp::RemovePosting {
                        key: ok.to_bytes(),
                        provenance,
                        owner: Some(*record_id.as_bytes()),
                    });
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        ops.push(IndexWriteOp::RemovePosting {
                            key: old_bytes,
                            provenance,
                            owner: Some(*record_id.as_bytes()),
                        });
                        ops.push(IndexWriteOp::SetPosting {
                            key: new_bytes,
                            value: Bytes::copy_from_slice(record_id.as_bytes()),
                            provenance,
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
        record_id: &RecordId,
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
                ops.push(IndexWriteOp::RemovePosting {
                    key: irk.to_bytes(),
                    provenance: unique_provenance(&def),
                    owner: Some(*record_id.as_bytes()),
                });
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
        let mut ops = Vec::with_capacity(1024);
        for def in self.indexes_unique.iter() {
            let provenance = unique_provenance(&def);
            for (rid, value) in items.clone() {
                if let Some(irk) =
                    build_index_key_from_record(true, def.name_interned, value, &def.paths)
                {
                    ops.push(IndexWriteOp::SetPosting {
                        key: irk.to_bytes(),
                        value: Bytes::copy_from_slice(rid.as_bytes()),
                        provenance,
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
