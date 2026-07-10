use bytes::Bytes;
use shamir_collections::TFxSet;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::RecordKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;
use crate::index::index_definition::IndexDefinition;

impl TableManager {
    // ---- index2 event hooks (used by crud + replication) ----

    /// Map an `IndexError` to a `DbError::Internal` for propagation.
    /// This ensures scalar-eval failures (e.g. user scalar not re-registered
    /// after reopen) surface as LOUD errors instead of being swallowed.
    fn io_err(e: shamir_index::IndexError) -> DbError {
        DbError::Internal(e.to_string())
    }

    pub(super) async fn index2_on_insert(&self, rid: &RecordId, rec: &InnerValue) -> DbResult<()> {
        for backend in self.index2_registry.all_backends().await {
            let ops = backend.plan_insert(*rid, rec).await.map_err(Self::io_err)?;
            crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref())
                .await
                .map_err(Self::io_err)?;
        }
        Ok(())
    }

    pub(super) async fn index2_on_update(
        &self,
        rid: &RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> DbResult<()> {
        for backend in self.index2_registry.all_backends().await {
            let ops = backend
                .plan_update(*rid, old, new)
                .await
                .map_err(Self::io_err)?;
            crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref())
                .await
                .map_err(Self::io_err)?;
        }
        Ok(())
    }

    pub(super) async fn index2_on_delete(&self, rid: &RecordId, rec: &InnerValue) -> DbResult<()> {
        for backend in self.index2_registry.all_backends().await {
            let ops = backend.plan_delete(*rid, rec).await.map_err(Self::io_err)?;
            crate::index2::apply_index_ops(&ops, &self.info_store, backend.as_ref())
                .await
                .map_err(Self::io_err)?;
        }
        Ok(())
    }

    // ---- public CRUD surface ----

    /// Insert an InnerValue, returns RecordId (with counter and index update)
    ///
    /// Validates unique indexes BEFORE insert, returns error if constraint violated.
    ///
    /// cancel-safe: NO — sequence is data-write → counter-bump → 3 index
    /// updates with no WAL marker around it. Cancellation between the
    /// data write (`self.table.insert`) and the index hooks leaves the
    /// data store with orphan records that the indexes don't see.
    /// F5a: single-record CRUD is best-effort no-WAL (recovery via the
    /// doctor's `repair()` pass); the WAL-covered path is the batch /
    /// implicit-tx route. Do NOT call this under `tokio::select!` or
    /// `tokio::time::timeout` — use the batch / implicit-tx path for
    /// WAL-covered writes.
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let (id, _version) = self.insert_returning_version(value).await?;
        Ok(id)
    }

    /// Like [`insert`](Self::insert) but also returns the MVCC version
    /// assigned by the underlying store (for changefeed version
    /// alignment). Returns `0` when no `MvccStore` is attached.
    pub(crate) async fn insert_returning_version(
        &self,
        value: &InnerValue,
    ) -> DbResult<(RecordId, u64)> {
        let _guard = if self.index_manager.has_unique_indexes() {
            Some(self.unique_write_lock.lock().await)
        } else {
            None
        };

        // 1. Validate unique indexes BEFORE write
        self.index_manager.validate_unique_for_create(value).await?;

        // 2. Write to table. Route through MvccStore (SSI / version cache
        //    + history archival under active snapshots) when one is
        //    attached; otherwise fall back to a direct data_store write.
        //    Pre-generating the RecordId here lets us use the keyed
        //    `set_versioned` path instead of `Table::insert`'s auto-key
        //    `data_store.insert`. The MvccStore writes to `main` (same
        //    physical layout as direct `set`), so observers reading via
        //    `data_store.get` see the new record identically.
        let id = RecordId::new();
        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;
        let version = if let Some(mvcc) = &self.mvcc_store {
            // task #532: `set_versioned` is `RecordKey`-keyed — build the key
            // inline from the rid (alloc-free for the 16-byte shape) instead of
            // the prior `id.to_bytes()` heap `Bytes` that the store then had to
            // convert back to `RecordKey` internally.
            mvcc.set_versioned(RecordKey::from_slice(id.as_bytes()), bytes)
                .await?
        } else {
            self.table
                .data_store()
                .set(RecordKey::from_slice(id.as_bytes()), bytes)
                .await?;
            0
        };
        self.counter.increment(1).await?;

        // 3. Update indexes AFTER write
        self.index_manager.on_record_created(&id, value).await?;
        self.index_manager
            .on_record_created_unique(&id, value)
            .await?;
        self.sorted_indexes
            .on_record_created(&id, value, version)
            .await?;
        self.index2_on_insert(&id, value).await?;

        // SSI footprint: record this non-tx insert so Serializable txs see it.
        let ssi_ops = self
            .sorted_indexes
            .plan_record_created(&id, value, version)
            .unwrap_or_default();
        self.record_nontx_ssi_footprint(version, &ssi_ops);

        Ok((id, version))
    }

    /// Batched insert of N records. Validates unique indexes first
    /// for all values, then issues one batched `Table::insert_many`
    /// (which dispatches to `Store::insert_many` — on nebari / persy /
    /// redb that's a single transaction = one fsync for the data
    /// store). Counter increments by N once; index updates still
    /// loop per-record (a follow-up sprint can batch the index
    /// writes through `info_store.set_many`).
    ///
    /// Atomicity matches `Store::insert_many` for the chosen backend
    /// (transactional all-or-nothing on nebari / persy / redb;
    /// per-record on backends using the default loop impl).
    pub async fn insert_many(&self, values: &[InnerValue]) -> DbResult<Vec<RecordId>> {
        let (ids, _version) = self.insert_many_returning_version(values).await?;
        Ok(ids)
    }

    /// Like [`insert_many`](Self::insert_many) but also returns the
    /// maximum MVCC version assigned across the batch (for changefeed
    /// version alignment). Returns `0` when no `MvccStore` is attached
    /// or the batch is empty.
    pub(crate) async fn insert_many_returning_version(
        &self,
        values: &[InnerValue],
    ) -> DbResult<(Vec<RecordId>, u64)> {
        if values.is_empty() {
            return Ok((Vec::new(), 0));
        }

        // 1. Validate unique indexes for every value first. Two
        //    layers of check: persisted state (via
        //    `validate_unique_for_create`) AND batch-local seen
        //    map, because two rows within ONE batch with the same
        //    unique value would otherwise both pass the persisted
        //    check and silently overwrite each other in step 3.
        if self.index_manager.has_unique_indexes() {
            // Snapshot unique-index defs ONCE per batch — they are stable for the
            // duration of insert_many (mutated only by DDL). Eliminates 2×N
            // DashMap-iter + N×IndexDefinition::clone seen on the hot path
            // (flamegraph: dashmap::Iter::next 3.2% + lock_shared 0.89%).
            let unique_defs: Vec<IndexDefinition> =
                self.index_manager.iter_unique_indexes().collect();
            // Map: (unique_index_name_interned, encoded_values_key)
            // → first index in the batch that claimed it. Cheap
            // bincode-based key avoids fighting `InnerValue` hash
            // requirements (Map keyed by interner ids isn't `Hash`).
            let mut batch_seen: TFxSet<(u64, Vec<u8>)> = TFxSet::default();
            for (i, v) in values.iter().enumerate() {
                self.index_manager
                    .validate_unique_for_create_with_defs(v, &unique_defs)
                    .await?;
                // Now record this row's unique-index claims so the
                // next iteration sees them.
                for def in &unique_defs {
                    if let Some(vs) = crate::index::index_keys::extract_index_leaves(v, &def.paths)
                    {
                        let key = bincode::serialize(&vs)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        if !batch_seen.insert((def.name_interned, key)) {
                            return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                                "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                                def.name_interned, i
                            )));
                        }
                    }
                }
            }
        }

        // 2. Data-store write. When an MvccStore is attached, route the
        //    whole batch through `set_versioned_many_append_only` (III.4) so
        //    `version_cache` and history archival stay consistent with
        //    non-tx writes WHILE collapsing the main writes into a single
        //    `Store::transact` — one fsync instead of N on backends that
        //    override `transact`. The previous per-record `set_versioned`
        //    loop re-introduced the N× fsync amplification this path now
        //    avoids. Without an MvccStore we keep the legacy batched path
        //    (one transaction = one fsync on backends that override
        //    `insert_many`).
        let (ids, batch_version): (Vec<RecordId>, u64) = if let Some(mvcc) = &self.mvcc_store {
            let mut ids = Vec::with_capacity(values.len());
            let mut items = Vec::with_capacity(values.len());
            for v in values {
                let rid = RecordId::new();
                let bytes = v.to_bytes().map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "Failed to serialize InnerValue: {}",
                        e
                    ))
                })?;
                // task #532: `set_versioned_many_append_only` is keyed on
                // `RecordKey` — build the key inline (alloc-free) rather than
                // `rid.to_bytes()` → store-internal `Bytes`→`RecordKey`.
                items.push((RecordKey::from_slice(rid.as_bytes()), bytes));
                ids.push(rid);
            }
            let ver = mvcc.set_versioned_many_append_only(items).await?;
            (ids, ver)
        } else {
            let ids = self.table.insert_many(values).await?;
            (ids, 0)
        };

        // F4b-4: the V1 per-table WAL marker (`begin_with_delta`/`commit`)
        // is GONE. This batched non-tx insert path is now reachable only via
        // the query_runner's non-tx branch — which routes through the implicit
        // Snapshot tx (`run_implicit_batch_tx` → `execute_insert_tx`) and emits
        // ONE `WalEntryV2` to the repo file WAL — and from tests. Crash
        // recoverability is therefore owned by the file WAL, not the V1 marker.
        // (`shamir_wal::WalManager` + V1 codec removed in F5c.)

        // 4. counter + indexes (all in info_store).
        self.counter.increment(ids.len() as i64).await?;
        let pairs_iter = || ids.iter().zip(values.iter());
        self.index_manager
            .on_records_created_batch(pairs_iter())
            .await?;
        self.index_manager
            .on_records_created_unique_batch(pairs_iter())
            .await?;
        self.sorted_indexes
            .on_records_created_batch(pairs_iter(), batch_version)
            .await?;
        for (id, value) in pairs_iter() {
            self.index2_on_insert(id, value).await?;
        }

        // 5. Bump the watchdog. Every AUTO_VERIFY_EVERY_N_WRITES
        //    operations a background verify fires and logs any
        //    inconsistency. Non-blocking, single-flight, best-
        //    effort signal.
        self.bump_write_counter(ids.len() as u64);

        // F4b-4: the SSI footprint for this batch is now recorded by the
        // implicit-tx commit path (`gate.record_commit_writes`), since the only
        // live caller (query_runner non-tx branch) routes through that tx. The
        // redundant `record_nontx_ssi_footprint` call was removed here.

        Ok((ids, batch_version))
    }

    /// Delete a record by RecordId (with counter and index update)
    ///
    /// cancel-safe: NO — data-delete → counter decrement → 3 index
    /// deletes without WAL coverage. Cancellation after the data delete
    /// but before the index hooks leaves orphan index entries (a record
    /// the data store no longer has but the indexes still point to).
    /// F5a: single-record CRUD is best-effort no-WAL (recovery via the
    /// doctor's `repair()` pass); the WAL-covered delete path is the
    /// batch / implicit-tx route (`run_implicit_batch_tx` +
    /// `execute_delete_tx`). Do NOT call this under `tokio::select!` or
    /// `tokio::time::timeout`.
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        let (removed, _version) = self.delete_returning_version(id).await?;
        Ok(removed)
    }

    /// Like [`delete`](Self::delete) but also returns the MVCC version
    /// assigned by the underlying store (for changefeed version
    /// alignment). Returns `0` when no `MvccStore` is attached or the
    /// record did not exist.
    pub(crate) async fn delete_returning_version(&self, id: RecordId) -> DbResult<(bool, u64)> {
        // Get old value before deletion for index cleanup
        let old_value = self.get(id).await.ok();
        // Route through MvccStore when attached so the old bytes are
        // archived to history under active snapshots and `version_cache`
        // is bumped.
        let (removed, version) = if let Some(mvcc) = &self.mvcc_store {
            if old_value.is_some() {
                // task #532: `delete_versioned` is `RecordKey`-keyed — inline.
                let v = mvcc
                    .delete_versioned(RecordKey::from_slice(id.as_bytes()))
                    .await?;
                (true, v)
            } else {
                (false, 0)
            }
        } else {
            let r = self.table.delete(id).await?;
            (r, 0)
        };
        if removed {
            self.counter.increment(-1).await?;
            if let Some(ref old) = old_value {
                self.index_manager.on_record_deleted(&id, old).await?;
                self.index_manager
                    .on_record_deleted_unique(&id, old)
                    .await?;
                self.sorted_indexes.on_record_deleted(&id, old).await?;
                self.index2_on_delete(&id, old).await?;
            }
            // SSI footprint: delete touches the table (coarse TableScan
            // detection). No new index postings for a delete.
            self.record_nontx_ssi_footprint(version, &[]);
        }
        Ok((removed, version))
    }

    /// Set a record by RecordId - creates if not exists, updates if exists (with counter and index update)
    ///
    /// Validates unique indexes BEFORE write, returns error if constraint violated.
    ///
    /// cancel-safe: NO — read-then-validate-then-write-then-index-update
    /// without WAL coverage. Cancellation between the table write and
    /// the index hooks leaves stale index entries (indexes point at the
    /// previous value while the data store holds the new one). Use the
    /// batch path (`execute_update` / `insert_many`) when atomicity
    /// matters; do NOT call this under `tokio::select!` or
    /// `tokio::time::timeout`.
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        let (created, _version) = self.set_returning_version(id, value).await?;
        Ok(created)
    }

    /// Like [`set`](Self::set) but also returns the MVCC version
    /// assigned by the underlying store (for changefeed version
    /// alignment). Returns `0` when no `MvccStore` is attached.
    pub(crate) async fn set_returning_version(
        &self,
        id: RecordId,
        value: &InnerValue,
    ) -> DbResult<(bool, u64)> {
        let _guard = if self.index_manager.has_unique_indexes() {
            Some(self.unique_write_lock.lock().await)
        } else {
            None
        };

        // Get old value before update for index maintenance
        let old_value = self.get(id).await.ok();

        // 1. Validate unique indexes BEFORE write
        if let Some(ref old) = old_value {
            self.index_manager
                .validate_unique_for_update(&id, old, value)
                .await?;
        } else {
            self.index_manager.validate_unique_for_create(value).await?;
        }

        // 2. Write to table. Route through MvccStore when attached so
        //    `version_cache` is updated for SSI conflict detection and
        //    the old bytes are archived to history under active snapshots.
        //    `created` is derived from the pre-read above (same semantics
        //    as the previous `self.table.set` which internally did the
        //    same exists-check).
        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;
        let created = old_value.is_none();
        let version = if let Some(mvcc) = &self.mvcc_store {
            // task #532: `set_versioned` is `RecordKey`-keyed — build the key
            // inline from the rid (alloc-free for the 16-byte shape) instead of
            // the prior `id.to_bytes()` heap `Bytes` that the store then had to
            // convert back to `RecordKey` internally.
            mvcc.set_versioned(RecordKey::from_slice(id.as_bytes()), bytes)
                .await?
        } else {
            self.table
                .data_store()
                .set(RecordKey::from_slice(id.as_bytes()), bytes)
                .await?;
            0
        };

        // 3. Update indexes AFTER write
        let ssi_ops = if created {
            self.counter.increment(1).await?;
            self.index_manager.on_record_created(&id, value).await?;
            self.index_manager
                .on_record_created_unique(&id, value)
                .await?;
            self.sorted_indexes
                .on_record_created(&id, value, version)
                .await?;
            self.index2_on_insert(&id, value).await?;
            self.sorted_indexes
                .plan_record_created(&id, value, version)
                .unwrap_or_default()
        } else if let Some(ref old) = old_value {
            self.index_manager
                .on_record_updated(&id, old, value)
                .await?;
            self.index_manager
                .on_record_updated_unique(&id, old, value)
                .await?;
            self.sorted_indexes
                .on_record_updated(&id, old, value, version)
                .await?;
            self.index2_on_update(&id, old, value).await?;
            self.sorted_indexes
                .plan_record_updated(&id, old, value, version)
                .unwrap_or_default()
        } else {
            vec![]
        };

        // SSI footprint: record this non-tx write so Serializable txs see it.
        self.record_nontx_ssi_footprint(version, &ssi_ops);

        Ok((created, version))
    }

    /// Count records (uses stored counter for O(1) performance)
    pub async fn count(&self) -> DbResult<usize> {
        Ok(self.counter.get().await? as usize)
    }

    /// Get a record by RecordId
    pub async fn get(&self, id: RecordId) -> DbResult<InnerValue> {
        if let Some(mvcc) = self.mvcc_store_ref() {
            match mvcc.get_current_bytes(id.as_bytes()).await? {
                Some(bytes) => InnerValue::from_bytes(bytes)
                    .map_err(|e| DbError::Codec(format!("Failed to deserialize InnerValue: {e}"))),
                None => Err(DbError::NotFound(format!("record not found: {id:?}"))),
            }
        } else {
            self.table.get(id).await
        }
    }

    /// Vectored current-version read through the seam. FINAL-A: when an
    /// MvccStore is attached, reads from the version log (`get_current`);
    /// otherwise falls through to the raw `table.get_many` (data_store).
    /// Returns `None` for a slot when the key is absent or tombstoned.
    pub async fn get_many(&self, ids: &[RecordId]) -> DbResult<Vec<Option<InnerValue>>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(mvcc) = self.mvcc_store_ref() {
            // L3: batched MVCC read — one history.get_many for warm keys.
            let batch_keys: Vec<Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
            let raw = mvcc.get_current_many(&batch_keys).await?;
            let mut out = Vec::with_capacity(raw.len());
            for slot in raw {
                match slot {
                    Some(bytes) => {
                        let v = InnerValue::from_bytes(bytes).map_err(|e| {
                            DbError::Codec(format!("Failed to deserialize InnerValue: {e}"))
                        })?;
                        out.push(Some(v));
                    }
                    None => out.push(None),
                }
            }
            Ok(out)
        } else {
            self.table.get_many(ids).await
        }
    }

    /// Byte-level single-record read — returns the raw storage bytes WITHOUT
    /// decoding to `InnerValue`. Callers wrap in `RecordView::new(&bytes)` for
    /// zero-copy field access. The `InnerValue`-decoding [`get`] is kept for
    /// the aggregate pipeline and other callers that need the full tree.
    pub async fn get_bytes(&self, id: RecordId) -> DbResult<Option<Bytes>> {
        if let Some(mvcc) = self.mvcc_store_ref() {
            mvcc.get_current_bytes(id.as_bytes()).await
        } else {
            match self
                .table
                .data_store()
                .get(RecordKey::from_slice(id.as_bytes()))
                .await
            {
                Ok(b) => Ok(Some(b)),
                Err(DbError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            }
        }
    }

    /// Vectored byte-level read — returns raw storage bytes for each id
    /// WITHOUT decoding to `InnerValue`. Returns `None` for missing/tombstoned
    /// keys. Callers wrap each `Bytes` in `RecordView::new(&bytes)` for
    /// zero-copy field access. The `InnerValue`-decoding [`get_many`] is kept
    /// for the aggregate pipeline.
    pub async fn get_many_bytes(&self, ids: &[RecordId]) -> DbResult<Vec<Option<Bytes>>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(mvcc) = self.mvcc_store_ref() {
            // L3: batched MVCC read — one history.get_many for warm keys.
            let batch_keys: Vec<Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
            mvcc.get_current_many(&batch_keys).await
        } else {
            let keys: Vec<RecordKey> = ids
                .iter()
                .map(|id| RecordKey::from_slice(id.as_bytes()))
                .collect();
            self.table.data_store().get_many(keys).await
        }
    }

    /// tx-aware vectored byte-level read for the filtered-ANN path (V3.1).
    ///
    /// Falls through to [`get_many_bytes`] when `tx` is `None`. When `tx` is
    /// `Some`, resolves each id against the tx's staging store FIRST (so
    /// staged inserts are visible as bytes), then the committed data store.
    /// A staged `Remove` yields `None` (the record is gone from this tx's
    /// view). This mirrors the `StagingStore::get` read-through contract but
    /// in vectored form.
    pub(crate) async fn get_many_bytes_tx(
        &self,
        ids: &[RecordId],
        tx: Option<&shamir_tx::TxContext>,
    ) -> DbResult<Vec<Option<Bytes>>> {
        // Fast path: no tx → plain vectored read.
        let tx = match tx {
            None => return self.get_many_bytes(ids).await,
            Some(t) => t,
        };
        let token = self.table_token();
        // Resolve the per-table staging store. When absent, this tx has no
        // staged writes for this table → plain vectored read.
        let staging_opt = tx.write_set.get(&token);
        match staging_opt {
            None => self.get_many_bytes(ids).await,
            Some(staging) => {
                // For each id: check staging first, then fall through to the
                // committed store. The staging probe is sync (TMap lookup),
                // so we batch the committed-store fallback into one get_many.
                let mut out: Vec<Option<Bytes>> = Vec::with_capacity(ids.len());
                let mut fallback_idxs: Vec<usize> = Vec::new();
                let mut fallback_keys: Vec<Bytes> = Vec::new();
                for (i, id) in ids.iter().enumerate() {
                    let key = id.to_bytes();
                    match staging.staged_op(&key) {
                        Some(shamir_tx::staging_store::StagedKind::Set(b)) => {
                            out.push(Some(b));
                        }
                        Some(shamir_tx::staging_store::StagedKind::Removed) => {
                            out.push(None);
                        }
                        None => {
                            out.push(None); // placeholder; resolved below
                            fallback_idxs.push(i);
                            fallback_keys.push(key);
                        }
                    }
                }
                // Batch-resolve the non-staged ids from the committed store.
                if !fallback_keys.is_empty() {
                    let committed = if let Some(mvcc) = self.mvcc_store_ref() {
                        mvcc.get_current_many(&fallback_keys).await?
                    } else {
                        // Boundary: `fallback_keys` is `Bytes` (also fed by-ref
                        // to the mvcc branch); `get_many` takes `RecordKey`.
                        let keys: Vec<RecordKey> =
                            fallback_keys.into_iter().map(Into::into).collect();
                        self.table.data_store().get_many(keys).await?
                    };
                    for (slot, bytes) in fallback_idxs.into_iter().zip(committed.into_iter()) {
                        out[slot] = bytes;
                    }
                }
                Ok(out)
            }
        }
    }
}
