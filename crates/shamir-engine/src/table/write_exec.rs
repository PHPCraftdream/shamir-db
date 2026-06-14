//! Write operation execution on TableManager.
//!
//! Implements execute_insert, execute_update, execute_delete for TableManager.

use std::borrow::Cow;
use std::cell::RefCell;
use std::time::Instant;

use futures::StreamExt;
use fxhash::FxHashMap;
use serde_json as json;

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::write::{
    DeleteOp, InsertOp, InsertedRecord, SetOp, UpdateOp, UpdateReturnMode, WriteResult,
};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    inner_to_json_value, query_value_to_inner, query_value_to_inner_with,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

use crate::validator::WriteOp;
use shamir_types::access::Actor;

use super::table_manager::TableManager;
use super::write_helpers::{
    intern_via_layered, make_layered_interner, resolve_computed_record,
    validator_failure_to_db_error,
};

impl TableManager {
    /// Execute an INSERT operation.
    ///
    /// Converts each QueryValue to InnerValue, inserts into the table,
    /// and returns the inserted records with their generated IDs.
    /// When `return_result` is `false` the per-record JSON map assembly
    /// is skipped entirely — `WriteResult::records` will be empty.
    pub async fn execute_insert(
        &self,
        op: &InsertOp,
        return_result: bool,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // 1. Resolve inline `$fn` computed fields first (fail-closed); the
        //    resolved record is what we both store and echo back, so the
        //    client never sees the unevaluated marker. Then convert all
        //    values to InnerValue upfront — any codec error fails the whole
        //    insert with nothing written (the first bad value aborts).
        // Per-batch intern cache (C1, non-tx): field names repeat across every
        // row in the batch. Without the cache, `touch_ind` does a DashMap
        // lookup per field per row — O(N×k) sharded-map operations. With the
        // cache we pay one DashMap lookup per unique field name per batch and
        // amortise the rest to an FxHashMap lookup — 3-5× cheaper for typical
        // short batches with uniform schema. Mirrors the identical pattern in
        // `execute_insert_tx` (Stage 11, commit fd90c44).
        let mut resolved_values: Vec<Cow<'_, QueryValue>> = Vec::with_capacity(op.values.len());
        let inner_values: Vec<InnerValue> = {
            let cache: RefCell<FxHashMap<String, InternerKey>> = RefCell::new(FxHashMap::default());
            let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
                {
                    let c = cache.borrow();
                    if let Some(ik) = c.get(key) {
                        return Ok(ik.clone());
                    }
                }
                let ik = interner.touch_ind(key).map(|t| t.into_key()).map_err(|e| {
                    shamir_types::codecs::CodecError::Decode(format!(
                        "Failed to intern key '{}': {}",
                        key, e
                    ))
                })?;
                cache.borrow_mut().insert(key.to_string(), ik.clone());
                Ok(ik)
            };
            let mut vals = Vec::with_capacity(op.values.len());
            for value in &op.values {
                let resolved = resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?;
                let inner = query_value_to_inner_with(&resolved, &intern_fn)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                vals.push(inner);
                resolved_values.push(resolved);
            }
            vals
        };

        // 2a. S3: run validators on each record (fail-closed, before
        //     persistence). Actor is System — the batch executor
        //     will thread the real actor once actor-carrying signatures
        //     land on execute_insert. // TODO actor threading
        for iv in &inner_values {
            self.run_validators(WriteOp::Insert, Some(iv), None, &Actor::System)
                .await
                .map_err(validator_failure_to_db_error)?;
        }

        // 2. One batched write — dispatches to `Store::insert_many` at
        //    the backend, collapsing N×fsync to 1×fsync on backends
        //    that have a native batched-write API (nebari, persy,
        //    redb). Index updates still loop per-record inside.
        let (ids, batch_version) = self.insert_many_returning_version(&inner_values).await?;

        // 3. Build the result records in input order (from the resolved
        //    values, so computed fields are echoed with their results).
        //    Skip entirely when the caller does not need the result back
        //    (fire-and-forget batch inserts with return_result=false).
        let records = if return_result {
            build_insert_result_records(&resolved_values, &ids)
        } else {
            Vec::new()
        };

        // Persist any newly interned keys.
        self.flush_metadata().await?;

        // Changefeed: emit one Put per inserted record. The event carries
        // the same MVCC version the data was written at (batch_version).
        let changes: Vec<_> = ids
            .iter()
            .zip(inner_values.iter())
            .filter_map(|(id, iv)| self.put_change(*id, iv))
            .collect();
        self.emit_nontx_changefeed(batch_version, changes);

        let affected = ids.len() as u64;
        Ok(WriteResult {
            affected,
            records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of [`execute_insert`](Self::execute_insert).
    ///
    /// Stages each insert through [`insert_tx`](Self::insert_tx) — no
    /// physical writes until `commit_tx` Phase 5. Returns the same
    /// `WriteResult` shape (records with `_id`, affected count).
    /// Interner / counter persistence is **skipped** — commit_tx
    /// handles that uniformly for all staged mutations.
    pub async fn execute_insert_tx(
        &self,
        op: &InsertOp,
        tx: &mut shamir_tx::TxContext,
        return_result: bool,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields first (fail-closed), then
        // intern field names through the tx overlay.
        //
        // Per-batch intern cache (C1): field names repeat across every row in
        // the batch (e.g. all 100 rows have "email", "city", "score"). Without
        // the cache, `touch_sync` does a DashMap lookup per field per row —
        // O(N×k) sharded-map operations. With the cache we pay one DashMap
        // lookup per unique field name per batch and amortise the rest to an
        // FxHashMap lookup — 3-5× cheaper for typical short batches with
        // uniform schema.
        let mut resolved_values: Vec<Cow<'_, QueryValue>> = Vec::with_capacity(op.values.len());
        let inner_values: Vec<InnerValue> = {
            let layered = make_layered_interner(interner, tx);
            let base_intern_fn = intern_via_layered(&layered);
            // RefCell lets the closure capture and mutate the cache while
            // satisfying the `Fn` (not `FnMut`) bound on `query_value_to_inner_with`.
            let cache: RefCell<FxHashMap<String, InternerKey>> = RefCell::new(FxHashMap::default());
            let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
                {
                    let c = cache.borrow();
                    if let Some(ik) = c.get(key) {
                        return Ok(ik.clone());
                    }
                }
                let ik = base_intern_fn(key)?;
                cache.borrow_mut().insert(key.to_string(), ik.clone());
                Ok(ik)
            };
            let mut vals = Vec::with_capacity(op.values.len());
            for value in &op.values {
                let resolved = resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?;
                let inner = query_value_to_inner_with(&resolved, &intern_fn)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                vals.push(inner);
                resolved_values.push(resolved);
            }
            vals
        };

        // S3: run validators on each record before staging.
        // tx path: resolve keys through the tx overlay so brand-new field
        // names (staged above into the layered interner, not yet in base)
        // resolve at validation time.
        // TODO actor threading — use Actor::System for now.
        for iv in &inner_values {
            self.run_validators_tx(WriteOp::Insert, Some(iv), None, &Actor::System, &*tx)
                .await
                .map_err(validator_failure_to_db_error)?;
        }

        // Batched tx insert — mirrors `insert_many`'s structure on the
        // tx staging path. Lifts per-row overhead
        // (`validate_unique_for_create`, `unique_keys_for`,
        // `all_backends().await`, legacy/sorted plan calls) out of
        // the row loop. Semantics identical to looping `insert_tx`:
        // same RecordId order, same unique-guard recording, same
        // staged_vectors, same counter delta.
        let ids: Vec<RecordId> = self.insert_tx_many(&inner_values, tx).await?;

        // Skip result-map assembly for fire-and-forget inserts
        // (return_result=false) — avoids per-row serde_json::Map build
        // and QueryValue→json::Value clone on the hot batch-insert path.
        let records = if return_result {
            build_insert_result_records(&resolved_values, &ids)
        } else {
            Vec::new()
        };

        let affected = ids.len() as u64;
        Ok(WriteResult {
            affected,
            records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Execute an UPDATE operation.
    ///
    /// Filters records by where_clause, merges `set` fields into each
    /// matched record, writes back. Returns affected count and optionally
    /// the updated records (controlled by UpdateSelect).
    pub async fn execute_update(
        &self,
        op: &UpdateOp,
        ctx: &FilterContext<'_>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields, then convert set fields to
        // InnerValue map entries. `$ref` resolves against the other literal
        // fields in the same `set` payload.
        let resolved_set = resolve_computed_record(&op.set, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        let set_inner = query_value_to_inner(&resolved_set, interner)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
        let set_map = match &set_inner {
            InnerValue::Map(m) => m,
            _ => {
                return Err(shamir_storage::error::DbError::Validation(
                    "UPDATE set must produce a Map".to_string(),
                ))
            }
        };

        // Collect matching records — Opt C: try index path first, fall
        // back to scan when no index plan applies.
        let matched = if let Some(ref filter) = op.where_clause {
            if let Some(via_index) = self.lookup_records_via_index(filter, ctx).await? {
                via_index
            } else {
                let callback = compile_filter(filter, interner);
                let mut result = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, record) in batch {
                        if callback.matches(&record, ctx) {
                            result.push((id, record));
                        }
                    }
                }
                result
            }
        } else {
            // No where clause — update ALL records (no index can help).
            let mut result = Vec::new();
            let stream = self.list_stream(batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                result.extend(batch_result?);
            }
            result
        };

        let mut affected: u64 = 0;
        let mut result_records: Vec<InsertedRecord> = Vec::with_capacity(matched.len());
        // Changefeed (Phase 3b follow-up): collect (id, new_value) for each
        // changed record so a Put can be emitted after the batch is durable.
        let mut changefeed_puts: Vec<(RecordId, InnerValue)> = Vec::with_capacity(matched.len());
        // Track the maximum MVCC version across per-record writes so the
        // changefeed event carries the batch's commit-version.
        let mut max_write_version: u64 = 0;
        let return_mode = op
            .select
            .as_ref()
            .map(|s| s.return_mode)
            .unwrap_or(UpdateReturnMode::Changed);
        let wants_records = op.select.is_some();

        // F4b-4: the V1 per-table WAL marker for the UPDATE batch is GONE.
        // This non-tx path is now reachable only via the query_runner's non-tx
        // branch (which routes through the implicit Snapshot tx →
        // `execute_update_tx` → one `WalEntryV2` to the repo file WAL) and from
        // tests, so the V1 marker is dead. Crash recoverability is owned by the
        // file WAL. (`shamir_wal::WalManager` removal is deferred to F5.)

        // Phase 1: merge + validate, collecting changed rows for batched write.
        let mut batch_pairs: Vec<(RecordId, InnerValue, InnerValue)> =
            Vec::with_capacity(matched.len());

        for (id, old_record) in &matched {
            let new_record = merge_inner_maps(old_record, set_map);
            let changed = &new_record != old_record;

            if changed {
                // S3: run validators before persisting.
                self.run_validators(
                    WriteOp::Update,
                    Some(&new_record),
                    Some(old_record),
                    &Actor::System,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                batch_pairs.push((*id, old_record.clone(), new_record.clone()));
                changefeed_puts.push((*id, new_record.clone()));
            }

            if wants_records {
                let should_include = match return_mode {
                    UpdateReturnMode::All => true,
                    UpdateReturnMode::Changed => changed,
                    UpdateReturnMode::Unchanged => !changed,
                };
                if should_include {
                    result_records.push(InsertedRecord::Json(inner_to_json_value(
                        &new_record,
                        interner,
                    )?));
                }
            }
        }

        // Phase 2: batched write — one MVCC transaction (one fsync).
        if !batch_pairs.is_empty() {
            let refs: Vec<(RecordId, &InnerValue, &InnerValue)> = batch_pairs
                .iter()
                .map(|(id, old, new_val)| (*id, old, new_val))
                .collect();
            let ver = self.update_many_returning_version(&refs).await?;
            max_write_version = max_write_version.max(ver);
            affected = batch_pairs.len() as u64;
        }

        self.bump_write_counter(affected);

        // Persist any newly interned keys (set fields may have new keys)
        if affected > 0 {
            self.flush_metadata().await?;
        }

        // Changefeed: emit one Put per changed record. The event carries
        // the max MVCC version from the per-record writes (best-effort).
        let changes: Vec<_> = changefeed_puts
            .iter()
            .filter_map(|(id, iv)| self.put_change(*id, iv))
            .collect();
        self.emit_nontx_changefeed(max_write_version, changes);

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of [`execute_update`](Self::execute_update).
    ///
    /// Filters records by `where_clause`, merges `set` fields, then
    /// stages each changed record via [`update_tx`](Self::update_tx).
    /// WAL is NOT opened here — `commit_tx` Phase 4 emits one V2
    /// entry covering the whole tx. Returns the same `WriteResult`
    /// shape. Interner / counter persistence is **skipped** —
    /// commit_tx handles that uniformly.
    pub async fn execute_update_tx(
        &self,
        op: &UpdateOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields, then intern field names
        // through the tx overlay.
        let resolved_set = resolve_computed_record(&op.set, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        let set_inner = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);
            query_value_to_inner_with(&resolved_set, &intern_fn)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
        };
        let set_map = match &set_inner {
            InnerValue::Map(m) => m,
            _ => {
                return Err(shamir_storage::error::DbError::Validation(
                    "UPDATE set must produce a Map".to_string(),
                ))
            }
        };

        let matched = if let Some(ref filter) = op.where_clause {
            if let Some(via_index) = self.lookup_records_via_index(filter, ctx).await? {
                via_index
            } else {
                let callback = compile_filter(filter, interner);
                let mut result = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, record) in batch {
                        if callback.matches(&record, ctx) {
                            result.push((id, record));
                        }
                    }
                }
                result
            }
        } else {
            let mut result = Vec::new();
            let stream = self.list_stream(batch_size);
            futures::pin_mut!(stream);
            while let Some(batch_result) = stream.next().await {
                result.extend(batch_result?);
            }
            result
        };

        let mut affected: u64 = 0;
        let mut result_records: Vec<InsertedRecord> = Vec::with_capacity(matched.len());
        let return_mode = op
            .select
            .as_ref()
            .map(|s| s.return_mode)
            .unwrap_or(UpdateReturnMode::Changed);
        let wants_records = op.select.is_some();

        for (id, old_record) in &matched {
            let new_record = merge_inner_maps(old_record, set_map);
            let changed = &new_record != old_record;

            if changed {
                // S3: run validators before staging (tx overlay-aware).
                // TODO actor threading — use Actor::System for now.
                self.run_validators_tx(
                    WriteOp::Update,
                    Some(&new_record),
                    Some(old_record),
                    &Actor::System,
                    &*tx,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                self.update_tx(*id, &new_record, Some(&mut *tx)).await?;
                affected += 1;
            }

            if wants_records {
                let should_include = match return_mode {
                    UpdateReturnMode::All => true,
                    UpdateReturnMode::Changed => changed,
                    UpdateReturnMode::Unchanged => !changed,
                };
                if should_include {
                    result_records.push(InsertedRecord::Json(inner_to_json_value(
                        &new_record,
                        interner,
                    )?));
                }
            }
        }

        Ok(WriteResult {
            affected,
            records: result_records,
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Execute a DELETE operation.
    ///
    /// Filters records by where_clause and deletes all matches.
    /// Returns the count of deleted records.
    pub async fn execute_delete(
        &self,
        op: &DeleteOp,
        ctx: &FilterContext<'_>,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Collect IDs to delete — Opt C: try index path, fall back to scan.
        let to_delete: Vec<RecordId> =
            if let Some(via_index) = self.lookup_records_via_index(&op.where_clause, ctx).await? {
                via_index.into_iter().map(|(id, _)| id).collect()
            } else {
                let callback = compile_filter(&op.where_clause, interner);
                let mut result = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, record) in batch {
                        if callback.matches(&record, ctx) {
                            result.push(id);
                        }
                    }
                }
                result
            };

        // F4b-4: this V1 per-table WAL marker is RETAINED (not dead). Unlike
        // INSERT/UPDATE, `execute_delete` still has LIVE non-runner callers —
        // `shamir-db` system_store (system_store.rs) and admin user/role
        // teardown (execute/admin_users_roles.rs) call it directly, NOT through
        // the implicit-tx query_runner path. Those callers rely on the V1
        // marker for crash recoverability, so it stays until F5 migrates the
        // single-record CRUD + direct-delete callers off the V1 codec.
        //
        // Open a WAL marker spanning the whole DELETE batch.
        // counter_delta is set pessimistically to -|to_delete| (the
        // ids we INTEND to delete). If some ids turn out not to
        // exist, recovery still works — the doctor's verify pass
        // would reconcile. Most realistic case: every id was just
        // looked up via index, so they all exist.
        let txn_id = if !to_delete.is_empty() {
            let id = self.wal().fresh_txn_id();
            self.wal()
                .begin_with_delta(
                    id,
                    shamir_wal::WalManager::ops_record_deleted(&to_delete),
                    -(to_delete.len() as i64),
                )
                .await?;
            Some(id)
        } else {
            None
        };

        // S3: run validators on each record before deleting.
        // Only fetch old records if there are delete-bound validators.
        let has_delete_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Delete))
        };
        if has_delete_validators && !to_delete.is_empty() {
            let records = self.get_many(&to_delete).await?;
            for rec in records.iter().flatten() {
                // TODO actor threading — use Actor::System for now.
                self.run_validators(WriteOp::Delete, None, Some(rec), &Actor::System)
                    .await
                    .map_err(validator_failure_to_db_error)?;
            }
        }

        let mut affected: u64 = 0;
        // Changefeed (Phase 3b follow-up): record the ids actually removed
        // (a `delete` that found no record contributes no event).
        let mut deleted_ids: Vec<RecordId> = Vec::with_capacity(to_delete.len());
        let mut max_write_version: u64 = 0;
        for id in to_delete {
            let (removed, ver) = self.delete_returning_version(id).await?;
            if removed {
                affected += 1;
                deleted_ids.push(id);
                max_write_version = max_write_version.max(ver);
            }
        }

        // Clear the WAL marker — DELETE batch durable.
        if let Some(id) = txn_id {
            self.wal().commit(id).await?;
        }
        self.bump_write_counter(affected);

        // Flush the counter cache (delete decremented it).
        if affected > 0 {
            self.flush_metadata().await?;
        }

        // Changefeed: emit one Delete per removed record. The event carries
        // the max MVCC version from the per-record deletes (best-effort).
        let changes: Vec<_> = deleted_ids
            .iter()
            .map(|id| self.delete_change(*id))
            .collect();
        self.emit_nontx_changefeed(max_write_version, changes);

        Ok(WriteResult {
            affected,
            records: Vec::new(),
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of [`execute_delete`](Self::execute_delete).
    ///
    /// Filters records by `where_clause`, stages each removal via
    /// [`delete_tx`](Self::delete_tx). WAL is NOT opened here —
    /// `commit_tx` Phase 4 emits one V2 entry covering the whole tx.
    pub async fn execute_delete_tx(
        &self,
        op: &DeleteOp,
        ctx: &FilterContext<'_>,
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        let to_delete: Vec<RecordId> =
            if let Some(via_index) = self.lookup_records_via_index(&op.where_clause, ctx).await? {
                via_index.into_iter().map(|(id, _)| id).collect()
            } else {
                let callback = compile_filter(&op.where_clause, interner);
                let mut result = Vec::new();
                let stream = self.list_stream(batch_size);
                futures::pin_mut!(stream);
                while let Some(batch_result) = stream.next().await {
                    let batch = batch_result?;
                    for (id, record) in batch {
                        if callback.matches(&record, ctx) {
                            result.push(id);
                        }
                    }
                }
                result
            };

        // S3: run validators on each record before deleting (tx).
        let has_delete_validators = {
            let bindings = self.validator_bindings();
            bindings.iter().any(|b| b.ops.contains(&WriteOp::Delete))
        };
        if has_delete_validators && !to_delete.is_empty() {
            let records = self.get_many(&to_delete).await?;
            for rec in records.iter().flatten() {
                // TODO actor threading — use Actor::System for now.
                self.run_validators_tx(WriteOp::Delete, None, Some(rec), &Actor::System, &*tx)
                    .await
                    .map_err(validator_failure_to_db_error)?;
            }
        }

        let mut affected: u64 = 0;
        for id in to_delete {
            if self.delete_tx(id, Some(&mut *tx)).await? {
                affected += 1;
            }
        }

        Ok(WriteResult {
            affected,
            records: Vec::new(),
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Execute a SET (upsert) operation.
    ///
    /// Finds an existing record by matching all key fields, then either
    /// updates it (merge) or inserts a new record if not found.
    pub async fn execute_set(&self, op: &SetOp) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Parse key as map → list of (field_path, value) to match
        // Use touch_ind (not get_ind) because key fields may not be interned yet
        let key_fields: Vec<(Vec<u64>, InnerValue)> = match &op.key {
            Value::Map(map) => {
                let mut fields = Vec::with_capacity(map.len());
                for (k, v) in map {
                    let key_id = match interner.touch_ind(k.as_str()) {
                        Ok(t) => t.key().id(),
                        Err(e) => return Err(shamir_storage::error::DbError::Codec(e.to_string())),
                    };
                    let inner_v = query_value_to_inner(v, interner)
                        .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                    fields.push((vec![key_id], inner_v));
                }
                fields
            }
            _ => {
                return Err(shamir_storage::error::DbError::Validation(
                    "SET key must be a JSON object".to_string(),
                ))
            }
        };

        // Locate the existing record (if any) — Opt B uses a regular
        // single-field index when one exists for the key path; falls
        // back to a full scan otherwise.
        let found = self
            .lookup_existing_for_set(&key_fields, batch_size)
            .await?;

        // Resolve inline `$fn` computed fields, then convert the new value.
        let resolved_value = resolve_computed_record(&op.value, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        let new_inner = query_value_to_inner(&resolved_value, interner)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;

        // Changefeed (Phase 3b follow-up): the upsert always ends in a Put of
        // the stored record — `(id, stored_value)` captured from whichever
        // branch ran (update-merge or fresh insert).
        let (created, write_version, result_record, changefeed_put) =
            if let Some((id, existing)) = found {
                // Update: merge new value into existing
                let new_map = match &new_inner {
                    InnerValue::Map(m) => m,
                    _ => {
                        return Err(shamir_storage::error::DbError::Validation(
                            "SET value must be a JSON object".to_string(),
                        ))
                    }
                };
                let merged = merge_inner_maps(&existing, new_map);

                // S3: run validators (Upsert — existing found → update path).
                // TODO actor threading — use Actor::System for now.
                self.run_validators(
                    WriteOp::Upsert,
                    Some(&merged),
                    Some(&existing),
                    &Actor::System,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                let (_was_created, ver) = self.set_returning_version(id, &merged).await?;
                let json = inner_to_json_value(&merged, interner)?;
                (false, ver, json, (id, merged))
            } else {
                // S3: run validators (Upsert — no existing → insert path).
                // TODO actor threading — use Actor::System for now.
                self.run_validators(WriteOp::Upsert, Some(&new_inner), None, &Actor::System)
                    .await
                    .map_err(validator_failure_to_db_error)?;

                // Insert new record
                let (id, ver) = self.insert_returning_version(&new_inner).await?;
                let mut obj = match inner_to_json_value(&new_inner, interner)? {
                    json::Value::Object(m) => m,
                    other => {
                        let mut m = json::Map::new();
                        m.insert("_value".to_string(), other);
                        m
                    }
                };
                obj.insert("_id".to_string(), json::Value::String(id.to_string()));
                (true, ver, json::Value::Object(obj), (id, new_inner.clone()))
            };

        let mut result_obj = match result_record {
            json::Value::Object(m) => m,
            other => {
                let mut m = json::Map::new();
                m.insert("_value".to_string(), other);
                m
            }
        };
        result_obj.insert("_created".to_string(), json::Value::Bool(created));

        // Persist any newly interned keys
        self.flush_metadata().await?;

        // Changefeed: emit the single Put. The event carries the MVCC
        // version the data was written at (best-effort, non-blocking).
        let (put_id, put_val) = changefeed_put;
        let changes: Vec<_> = self.put_change(put_id, &put_val).into_iter().collect();
        self.emit_nontx_changefeed(write_version, changes);

        Ok(WriteResult {
            affected: 1,
            records: vec![InsertedRecord::Json(json::Value::Object(result_obj))],
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }

    /// tx-aware variant of [`execute_set`](Self::execute_set).
    ///
    /// Stage 4.D.6.c.2: mirrors the upsert semantics of
    /// `execute_set` — locate by key fields, then either
    /// [`update_tx`](Self::update_tx) (merge) or
    /// [`insert_tx`](Self::insert_tx) (new record). Full parity with
    /// `execute_set` including index fast-path; differs only in that
    /// mutations go through tx-aware methods and interner / counter
    /// persistence is **skipped** (commit_tx handles that uniformly).
    pub async fn execute_set_tx(
        &self,
        op: &SetOp,
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let batch_size = 1000;
        let interner = self.interner().get().await?;

        // Resolve inline `$fn` computed fields in the value first (fail-closed).
        let resolved_value = resolve_computed_record(&op.value, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;

        // Intern field names through the tx overlay, then release the
        // immutable borrow on `tx` before the mutable `update_tx`/`insert_tx` calls.
        let (key_fields, new_inner) = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);

            let key_fields: Vec<(Vec<u64>, InnerValue)> = match &op.key {
                Value::Map(map) => {
                    let mut fields = Vec::with_capacity(map.len());
                    for (k, v) in map {
                        let key_id = layered.touch_sync(k.as_str());
                        let inner_v = query_value_to_inner_with(v, &intern_fn)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        fields.push((vec![key_id], inner_v));
                    }
                    fields
                }
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET key must be a JSON object".to_string(),
                    ))
                }
            };

            let new_inner = query_value_to_inner_with(&resolved_value, &intern_fn)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;

            (key_fields, new_inner)
        };

        let found = self
            .lookup_existing_for_set(&key_fields, batch_size)
            .await?;

        let (created, result_record) = if let Some((id, existing)) = found {
            let new_map = match &new_inner {
                InnerValue::Map(m) => m,
                _ => {
                    return Err(shamir_storage::error::DbError::Validation(
                        "SET value must be a JSON object".to_string(),
                    ))
                }
            };
            let merged = merge_inner_maps(&existing, new_map);

            // S3: run validators (Upsert — existing found → update path, tx).
            // TODO actor threading — use Actor::System for now.
            self.run_validators_tx(
                WriteOp::Upsert,
                Some(&merged),
                Some(&existing),
                &Actor::System,
                &*tx,
            )
            .await
            .map_err(validator_failure_to_db_error)?;

            self.update_tx(id, &merged, Some(&mut *tx)).await?;
            // Build result JSON from op.value merged onto the existing record
            // (existing was committed before this tx → base ids only, safe to decode).
            let mut base_obj = match inner_to_json_value(&existing, interner)? {
                json::Value::Object(m) => m,
                _ => json::Map::new(),
            };
            // Overlay op.value fields (QueryValue → serde_json::Value).
            if let Value::Map(overlay) = &op.value {
                for (k, v) in overlay {
                    base_obj.insert(k.clone(), json::Value::from(v.clone()));
                }
            }
            (false, json::Value::Object(base_obj))
        } else {
            // S3: run validators (Upsert — no existing → insert path, tx).
            // TODO actor threading — use Actor::System for now.
            self.run_validators_tx(
                WriteOp::Upsert,
                Some(&new_inner),
                None,
                &Actor::System,
                &*tx,
            )
            .await
            .map_err(validator_failure_to_db_error)?;

            let id = self.insert_tx(&new_inner, Some(&mut *tx)).await?;
            // Build result JSON from original op.value to avoid overlay-id
            // reverse-lookup (overlay ids are not yet in the base interner).
            let mut obj = match json::Value::from(op.value.clone()) {
                json::Value::Object(m) => m,
                other => {
                    let mut m = json::Map::new();
                    m.insert("_value".to_string(), other);
                    m
                }
            };
            obj.insert("_id".to_string(), json::Value::String(id.to_string()));
            (true, json::Value::Object(obj))
        };

        let mut result_obj = match result_record {
            json::Value::Object(m) => m,
            other => {
                let mut m = json::Map::new();
                m.insert("_value".to_string(), other);
                m
            }
        };
        result_obj.insert("_created".to_string(), json::Value::Bool(created));

        Ok(WriteResult {
            affected: 1,
            records: vec![InsertedRecord::Json(json::Value::Object(result_obj))],
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}

/// Build the `Vec<InsertedRecord>` result for an INSERT response.
///
/// Returns `Direct` variants — no `serde_json::Map` is allocated per row.
/// The serialiser emits the same msgpack map shape as the old `Json` path.
fn build_insert_result_records(
    resolved_values: &[std::borrow::Cow<'_, QueryValue>],
    ids: &[RecordId],
) -> Vec<InsertedRecord> {
    resolved_values
        .iter()
        .zip(ids.iter())
        .map(|(value, id)| InsertedRecord::Direct {
            id: *id,
            fields: (**value).clone(),
        })
        .collect()
}

/// Merge set_map fields into an existing InnerValue record.
///
/// Only works on Map values. For each key in set_map, overwrite
/// the corresponding key in the original. Keys not in set_map
/// are preserved.
fn merge_inner_maps(
    original: &InnerValue,
    set_map: &shamir_types::types::common::TMap<InternerKey, InnerValue>,
) -> InnerValue {
    match original {
        InnerValue::Map(orig_map) => {
            let mut merged = orig_map.clone();
            for (key, value) in set_map {
                merged.insert(key.clone(), value.clone());
            }
            InnerValue::Map(merged)
        }
        _ => original.clone(),
    }
}
