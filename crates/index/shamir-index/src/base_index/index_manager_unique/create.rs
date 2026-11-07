use super::validate::fire_post_flag_set_pre_gen_bump_test_hook;
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::build_index_key_from_record;
use crate::base_index::index_manager::IndexManager;
use crate::base_index::write_barrier_flags::UNIQUE_INDEX_EXISTS;
use crate::model::identity::identity_bytes::{encode_unique_posting_value, INLINE_MAX};
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

impl IndexManager {
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
        self.create_unique_index_from_records_borrowed(index_def, &records)
            .await
    }

    /// Backfill from a borrowed snapshot, retaining the owned API's validation and publication order.
    pub async fn create_unique_index_from_records_borrowed(
        &self,
        index_def: IndexDefinition,
        records: &[(RecordId, InnerValue)],
    ) -> DbResult<()> {
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

        // Collect (record_id, index_key_bytes, canonical C) for duplicate
        // detection and bulk write. Duplicate detection is on the exact v2
        // key bytes — injective on equality classes for the inline form; a
        // digest-form duplicate within one build shares both the digest key
        // and (failing the build here) is never written with a foreign tail.
        let mut key_counts: TMap<Bytes, (usize, Vec<u8>)> = new_map();
        let mut entries: Vec<(RecordId, Bytes, Vec<u8>)> = Vec::with_capacity(16);

        for (record_id, value) in records {
            if let Some((irk, canonical)) =
                build_index_key_from_record(true, name_interned, value, &index_def.paths)
            {
                let index_key = irk.to_bytes();
                let prior = key_counts.get_mut(&index_key);
                if let Some((count, existing_canonical)) = prior {
                    if existing_canonical != &canonical {
                        let detail = format!(
                            "unique index identity digest collision during creation for index {name_interned}"
                        );
                        log::error!("{}", detail);
                        return Err(shamir_storage::error::DbError::Internal(detail));
                    }
                    *count += 1;
                } else {
                    key_counts.insert(index_key.clone(), (1, canonical.clone()));
                }
                entries.push((*record_id, index_key, canonical));
            }
        }

        let duplicates: Vec<(&Bytes, &(usize, Vec<u8>))> = key_counts
            .iter()
            .filter(|(_, (count, _))| *count > 1)
            .collect();
        if !duplicates.is_empty() {
            let duplicate_record_count: usize =
                duplicates.iter().map(|(_, (count, _))| *count).sum();
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
        // Values: owner-only for the inline form, owner + `C` tail for the
        // digest form (design (a)).
        let mut writes: Vec<(RecordKey, Bytes)> = Vec::with_capacity(count);
        for (record_id, index_key, canonical) in entries {
            let digest = canonical.len() > INLINE_MAX;
            let value = Bytes::from(encode_unique_posting_value(
                &record_id,
                digest.then_some(canonical.as_slice()),
            ));
            writes.push((index_key.into(), value));
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
            let detail = format!(
                "CREATE UNIQUE INDEX '{name_interned}': the index posting \
                 entries were durably written, but persisting the index \
                 definition failed: {e}. The index definition is NOT persisted — \
                 on restart, orphan postings will exist without a corresponding \
                 definition. Call TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
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
                let detail = format!(
                    "DROP UNIQUE INDEX '{name_interned}': a durable drop tombstone \
                 was persisted and the definition was retired from the planner, \
                 but the posting sweep failed: {e}. On restart, recovery will \
                 resume the sweep idempotently and finish the drop. Call \
                 TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
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
                let detail = format!(
                    "DROP UNIQUE INDEX '{name_interned}': a durable drop \
                     tombstone was persisted, the definition was retired, and \
                     the posting sweep completed, but persisting the reduced \
                     index metadata failed: {e}. On restart, recovery will \
                     finish the drop idempotently. Call TableManager::verify() \
                     to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
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
                        completed_at: crate::base_index::ddl_op_log::timestamp_millis(
                            std::time::SystemTime::now(),
                        ),
                    },
                };
                // Do not swallow status-write errors — log loudly.
                if let Err(e) =
                    crate::base_index::ddl_op_log::write_op_status(&self.info_store, &status).await
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
                let detail = format!(
                    "DROP UNIQUE INDEX '{name_interned}': the drop is essentially \
                 complete (tombstone persisted, definition retired, sweep done, \
                 reduced metadata persisted), but clearing the drop tombstone \
                 failed: {e}. On restart, recovery will clear the tombstone as \
                 a no-op. Call TableManager::verify() to confirm state."
                );
                log::error!("{}", detail);
                crate::backend::contextualize_db_error(e, detail)
            })?;

        Ok(was_removed)
    }

    /// Сохраняет метаданные уникальных индексов в служебное хранилище.
    pub(in crate::base_index) async fn save_index_info_unique(&self) -> DbResult<()> {
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
}
