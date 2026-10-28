use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::build_index_key_from_record;
use crate::base_index::index_manager::IndexManager;
use crate::write_ops::IndexWriteOp;
use bytes::Bytes;
use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_tx::{IndexFamily, Provenance};
use shamir_types::record_view::RecordRef;
use shamir_types::types::record_id::RecordId;

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
pub(super) async fn fire_post_flag_set_pre_gen_bump_test_hook() {
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
        let defs = self.indexes_unique.snapshot();
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
    /// a write (Set or Remove) for, per `is_record_touched` — the caller
    /// provides a closure that answers "is this specific record ID touched
    /// by this tx?" on demand, avoiding the O(N) cost of materializing a full
    /// set of all touched records before any check even runs.
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
    /// for, so `is_record_touched` correctly rejects the tolerance and this
    /// call falls through to `DuplicateKey`.
    ///
    /// The on-demand closure pattern mirrors the O(1) probe in
    /// `pre_commit.rs` Step 2: `tx.write_set.get(&table_token).is_some_and(|s|
    /// s.staged_op(candidate_id.as_bytes()).is_some())`. The closure is only
    /// called when a durable conflict is found (after `check_unique_key`
    /// returns `Some(existing_id)`), which is rare per-row — no O(N) set
    /// build is needed when there are no conflicts.
    pub async fn validate_unique_for_create_with_released<F>(
        &self,
        value: &(impl RecordRef + ?Sized),
        released_in_tx: &shamir_collections::TFxSet<Vec<u8>>,
        is_record_touched: F,
    ) -> DbResult<()>
    where
        F: Fn(&RecordId) -> bool,
    {
        if !self.has_unique_indexes() {
            return Ok(());
        }
        let defs = self.indexes_unique.snapshot();
        for def in defs.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                let index_key = irk.to_bytes();
                if let Some(existing_id) = self.check_unique_key(&index_key).await? {
                    if released_in_tx.contains(index_key.as_ref())
                        && is_record_touched(&existing_id)
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

        let defs = self.indexes_unique.snapshot();
        for def in defs.iter() {
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
    ///
    /// The on-demand closure pattern mirrors the O(1) probe in
    /// `pre_commit.rs` Step 2 and `validate_unique_for_create_with_released`:
    /// `tx.write_set.get(&table_token).is_some_and(|s|
    /// s.staged_op(candidate_id.as_bytes()).is_some())`. The closure is only
    /// called when a durable conflict is found (after `check_unique_key`
    /// returns `Some(existing_id)`), which is rare per-row.
    pub async fn validate_unique_for_update_with_released<F>(
        &self,
        record_id: &RecordId,
        old_value: &(impl RecordRef + ?Sized),
        new_value: &(impl RecordRef + ?Sized),
        released_in_tx: &shamir_collections::TFxSet<Vec<u8>>,
        is_record_touched: F,
    ) -> DbResult<()>
    where
        F: Fn(&RecordId) -> bool,
    {
        if !self.has_unique_indexes() {
            return Ok(());
        }

        let defs = self.indexes_unique.snapshot();
        for def in defs.iter() {
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
                        && is_record_touched(&existing_id)
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

    pub(in crate::base_index) async fn check_unique_key(
        &self,
        index_key: &Bytes,
    ) -> DbResult<Option<RecordId>> {
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

        let defs = self.indexes_unique.snapshot();
        let mut ops: Vec<KvOp> = Vec::with_capacity(defs.len());
        for def in defs.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                ops.push(KvOp::Set(irk.to_bytes().into(), record_id.to_bytes()));
            }
        }
        if !ops.is_empty() {
            self.info_store.transact(ops).await?;
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

        let defs = self.indexes_unique.snapshot();
        let mut ops: Vec<KvOp> = Vec::with_capacity(defs.len() * 2);
        for def in defs.iter() {
            let old_key =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths);
            let new_key =
                build_index_key_from_record(true, def.name_interned, new_value, &def.paths);

            // Mixed remove-then-set order is preserved per def: last-write-wins
            // on the same key, and remove-then-set never swaps to set-then-remove.
            match (old_key, new_key) {
                (None, None) => {}
                (None, Some(nk)) => {
                    ops.push(KvOp::Set(nk.to_bytes().into(), record_id.to_bytes()));
                }
                (Some(ok), None) => {
                    ops.push(KvOp::Remove(ok.to_bytes().into()));
                }
                (Some(ok), Some(nk)) => {
                    let old_bytes = ok.to_bytes();
                    let new_bytes = nk.to_bytes();
                    if old_bytes != new_bytes {
                        ops.push(KvOp::Remove(old_bytes.into()));
                        ops.push(KvOp::Set(new_bytes.into(), record_id.to_bytes()));
                    }
                }
            }
        }
        if !ops.is_empty() {
            self.info_store.transact(ops).await?;
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

        let defs = self.indexes_unique.snapshot();
        let mut ops: Vec<KvOp> = Vec::with_capacity(defs.len());
        for def in defs.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths)
            {
                ops.push(KvOp::Remove(irk.to_bytes().into()));
            }
        }
        if !ops.is_empty() {
            self.info_store.transact(ops).await?;
        }

        Ok(())
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
        let defs = self.indexes_unique.snapshot();
        let mut ops = Vec::new();
        for def in defs.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, value, &def.paths)
            {
                ops.push(IndexWriteOp::SetPosting {
                    key: irk.to_bytes(),
                    value: Bytes::copy_from_slice(record_id.as_bytes()),
                    provenance: unique_provenance(def),
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
        let defs = self.indexes_unique.snapshot();
        let mut ops = Vec::new();
        for def in defs.iter() {
            let provenance = unique_provenance(def);
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
        let defs = self.indexes_unique.snapshot();
        let mut ops = Vec::new();
        for def in defs.iter() {
            if let Some(irk) =
                build_index_key_from_record(true, def.name_interned, old_value, &def.paths)
            {
                ops.push(IndexWriteOp::RemovePosting {
                    key: irk.to_bytes(),
                    provenance: unique_provenance(def),
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
        let defs = self.indexes_unique.snapshot();
        let mut ops = Vec::with_capacity(1024);
        for def in defs.iter() {
            let provenance = unique_provenance(def);
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
