//! Write operation execution on TableManager.
//!
//! Implements execute_insert, execute_update, execute_delete for TableManager.

use std::collections::BTreeSet;
use std::time::Instant;

use futures::StreamExt;
use serde_json as json;

use crate::query::filter::eval::resolve_field;
use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::Filter;
use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, WriteResult};
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{inner_to_json_value, json_value_to_inner};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;

impl TableManager {
    /// Execute an INSERT operation.
    ///
    /// Converts each JSON value to InnerValue, inserts into the table,
    /// and returns the inserted records with their generated IDs.
    pub async fn execute_insert(&self, op: &InsertOp) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // 1. Convert all JSON values to InnerValue upfront. Any
        //    codec error fails the whole insert with nothing written
        //    (matches the previous per-record semantics — the first
        //    bad value aborts).
        let mut inner_values: Vec<InnerValue> = Vec::with_capacity(op.values.len());
        for value in &op.values {
            let inner = json_value_to_inner(value, interner)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
            inner_values.push(inner);
        }

        // 2. One batched write — dispatches to `Store::insert_many` at
        //    the backend, collapsing N×fsync to 1×fsync on backends
        //    that have a native batched-write API (nebari, persy,
        //    redb). Index updates still loop per-record inside.
        let ids = self.insert_many(&inner_values).await?;

        // 3. Build the result records in input order.
        let mut records = Vec::with_capacity(op.values.len());
        for (value, id) in op.values.iter().zip(ids.iter()) {
            let mut obj = match value {
                json::Value::Object(map) => map.clone(),
                _ => json::Map::new(),
            };
            obj.insert("_id".to_string(), json::Value::String(id.to_string()));
            records.push(json::Value::Object(obj));
        }

        // Persist any newly interned keys.
        self.interner().persist().await?;
        self.counter().persist().await?;

        let affected = records.len() as u64;
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
    ) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        let mut inner_values: Vec<InnerValue> = Vec::with_capacity(op.values.len());
        for value in &op.values {
            let inner = json_value_to_inner(value, interner)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
            inner_values.push(inner);
        }

        let mut ids: Vec<RecordId> = Vec::with_capacity(inner_values.len());
        for v in &inner_values {
            let id = self.insert_tx(v, Some(&mut *tx)).await?;
            ids.push(id);
        }

        let mut records = Vec::with_capacity(op.values.len());
        for (value, id) in op.values.iter().zip(ids.iter()) {
            let mut obj = match value {
                json::Value::Object(map) => map.clone(),
                _ => json::Map::new(),
            };
            obj.insert("_id".to_string(), json::Value::String(id.to_string()));
            records.push(json::Value::Object(obj));
        }

        let affected = records.len() as u64;
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

        // Convert set fields to InnerValue map entries
        let set_inner = json_value_to_inner(&op.set, interner)
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
        let mut result_records: Vec<json::Value> = Vec::new();
        let return_mode = op
            .select
            .as_ref()
            .map(|s| s.return_mode)
            .unwrap_or(UpdateReturnMode::Changed);
        let wants_records = op.select.is_some();

        // Open a WAL marker for the whole UPDATE batch. counter_delta
        // is 0 because UPDATE doesn't change row count. Recovery for
        // RecordUpdated reapplies index hooks idempotently against
        // the current record value (post-restart it sees either the
        // pre- or post-update value, depending on where the crash
        // hit, but either is consistent with itself).
        //
        // Marker covers EVERY matched id, even ones whose merge
        // turned out to be a no-op (`changed == false`) — keeps the
        // recovery scope correct if a crash hit mid-merge.
        let candidate_ids: Vec<RecordId> = matched.iter().map(|(id, _)| *id).collect();
        let txn_id = if !candidate_ids.is_empty() {
            let id = self.wal().fresh_txn_id();
            self.wal()
                .begin_with_delta(
                    id,
                    shamir_wal::WalManager::ops_record_updated(&candidate_ids),
                    0,
                )
                .await?;
            Some(id)
        } else {
            None
        };

        for (id, old_record) in &matched {
            // Merge: overlay set_map onto existing record
            let new_record = merge_inner_maps(old_record, set_map);
            let changed = &new_record != old_record;

            if changed {
                self.set(*id, &new_record).await?;
                affected += 1;
            }

            if wants_records {
                let should_include = match return_mode {
                    UpdateReturnMode::All => true,
                    UpdateReturnMode::Changed => changed,
                    UpdateReturnMode::Unchanged => !changed,
                };
                if should_include {
                    result_records.push(inner_to_json_value(&new_record, interner)?);
                }
            }
        }

        // Clear the WAL marker — the UPDATE batch is durable.
        if let Some(id) = txn_id {
            self.wal().commit(id).await?;
        }
        self.bump_write_counter(affected);

        // Persist any newly interned keys (set fields may have new keys)
        if affected > 0 {
            self.interner().persist().await?;
            self.counter().persist().await?;
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

        let mut affected: u64 = 0;
        for id in to_delete {
            if self.delete(id).await? {
                affected += 1;
            }
        }

        // Clear the WAL marker — DELETE batch durable.
        if let Some(id) = txn_id {
            self.wal().commit(id).await?;
        }
        self.bump_write_counter(affected);

        // Flush the counter cache (delete decremented it).
        if affected > 0 {
            self.counter().persist().await?;
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

        // Parse key as JSON object → list of (field_path, value) to match
        // Use touch_ind (not get_ind) because key fields may not be interned yet
        let key_fields: Vec<(Vec<u64>, InnerValue)> = match &op.key {
            json::Value::Object(map) => {
                let mut fields = Vec::with_capacity(map.len());
                for (k, v) in map {
                    let key_id = match interner.touch_ind(k.as_str()) {
                        Ok(t) => t.key().id(),
                        Err(e) => return Err(shamir_storage::error::DbError::Codec(e.to_string())),
                    };
                    let inner_v = json_value_to_inner(v, interner)
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

        // Convert the new value
        let new_inner = json_value_to_inner(&op.value, interner)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;

        let (created, result_record) = if let Some((id, existing)) = found {
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
            self.set(id, &merged).await?;
            (false, inner_to_json_value(&merged, interner)?)
        } else {
            // Insert new record
            let id = self.insert(&new_inner).await?;
            let mut obj = match inner_to_json_value(&new_inner, interner)? {
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

        // Persist any newly interned keys
        self.interner().persist().await?;
        self.counter().persist().await?;

        Ok(WriteResult {
            affected: 1,
            records: vec![json::Value::Object(result_obj)],
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}

impl TableManager {
    /// **Opt C (write-path index planning).** Try to satisfy `filter`
    /// via an index lookup the same way `read` does — same planner
    /// (`try_plan_index_scan`), same residual-filter handling.
    ///
    /// Returns:
    ///   - `Ok(Some(records))` — index plan applied, returns the
    ///     matching records (already filtered by residual if any)
    ///   - `Ok(None)` — no index plan applies, caller must do a scan
    ///   - `Err(_)` — actual storage / planner error
    ///
    /// Used by `execute_update` and `execute_delete` to short-circuit
    /// the previous always-scan behaviour when a covering index exists.
    async fn lookup_records_via_index(
        &self,
        filter: &Filter,
        ctx: &FilterContext<'_>,
    ) -> DbResult<Option<Vec<(RecordId, InnerValue)>>> {
        let interner = self.interner().get().await?;
        let Some((idx_name, lookup_sets, residual)) = self.try_plan_index_scan(filter, interner)
        else {
            return Ok(None);
        };

        // Union the matching record IDs across every value-set the
        // planner produced (Eq → 1 set, In → N sets).
        let mut record_ids: BTreeSet<RecordId> = BTreeSet::new();
        for values in lookup_sets {
            let ids = self
                .index_manager_ref()
                .lookup_by_index(idx_name, &values)
                .await?;
            record_ids.extend(ids);
        }

        // Compile the residual filter once (if any) so we evaluate it
        // per-record without re-compilation.
        let residual_cb: Option<FilterNode> =
            residual.as_ref().map(|f| compile_filter(f, interner));

        let mut result = Vec::with_capacity(record_ids.len());
        for id in record_ids {
            let record = self.get(id).await?;
            let matches = match &residual_cb {
                Some(cb) => cb.matches(&record, ctx),
                None => true,
            };
            if matches {
                result.push((id, record));
            }
        }
        Ok(Some(result))
    }

    /// Locate the record matching `key_fields` for `execute_set`.
    ///
    /// **Opt B (write-path index lookup).** If the key has exactly one
    /// field AND there is a regular single-field index covering it,
    /// the lookup goes through `IndexManager::lookup_by_index` —
    /// O(log n) instead of the O(n) full-table scan that the original
    /// implementation always did.
    ///
    /// Falls back to the original full scan when:
    ///   - the key has more than one field (composite index lookup is
    ///     a future extension), or
    ///   - no matching index exists.
    async fn lookup_existing_for_set(
        &self,
        key_fields: &[(Vec<u64>, InnerValue)],
        batch_size: usize,
    ) -> DbResult<Option<(shamir_types::types::record_id::RecordId, InnerValue)>> {
        // Index fast-path — single-field key with a covering regular index.
        if key_fields.len() == 1 {
            let (path, value) = &key_fields[0];
            if let Some(idx_name) = self.find_single_field_index(path) {
                let ids = self
                    .index_manager_ref()
                    .lookup_by_index(idx_name, std::slice::from_ref(value))
                    .await?;
                if let Some(&id) = ids.iter().next() {
                    let inner = self.get(id).await?;
                    return Ok(Some((id, inner)));
                }
                // Index says: no record with this key → don't scan,
                // INSERT path.
                return Ok(None);
            }
        }

        // Fallback: full scan. Worst-case O(n); short-circuits on the
        // first match.
        let stream = self.list_stream(batch_size);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, record) in batch {
                let all_match = key_fields.iter().all(|(path, expected)| {
                    resolve_field(&record, path)
                        .map(|v| {
                            crate::query::filter::compare_values(&v, expected)
                                == Some(std::cmp::Ordering::Equal)
                        })
                        .unwrap_or(false)
                });
                if all_match {
                    return Ok(Some((id, record)));
                }
            }
        }
        Ok(None)
    }
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
