//! Write operation execution on TableManager.
//!
//! Implements execute_insert, execute_update, execute_delete for TableManager.

use std::collections::BTreeSet;
use std::time::Instant;

use futures::StreamExt;
use serde_json as json;

use crate::function::builtin_scalars;
use crate::query::filter::eval::resolve_field;
use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::write::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, WriteResult};
use shamir_funclib::registry::ScalarRegistry;
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{
    inner_to_json_value, json_value_to_inner, json_value_to_inner_with,
};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::validator::{ValidatorFailure, WriteOp};
use shamir_types::access::Actor;

use super::table_manager::TableManager;

/// Convert a [`ValidatorFailure`] into a [`DbError`](shamir_storage::error::DbError).
fn validator_failure_to_db_error(failure: ValidatorFailure) -> shamir_storage::error::DbError {
    match failure {
        ValidatorFailure::Failed(errors) => {
            // Serialize the structured errors as a JSON array so the
            // caller (and eventually the wire layer) gets field-bound
            // codes. The `ValidationError` derives `Serialize`.
            let json = serde_json::to_string(&errors).unwrap_or_else(|_| format!("{errors:?}"));
            shamir_storage::error::DbError::ValidatorRejected(json)
        }
        ValidatorFailure::Missing { id } => shamir_storage::error::DbError::ValidatorInvalid(
            format!("validator {} not found in registry (fail-closed)", id),
        ),
        ValidatorFailure::Invocation { id, reason } => {
            shamir_storage::error::DbError::ValidatorInvalid(format!(
                "validator {} invocation failed: {}",
                id, reason
            ))
        }
    }
}

// ============================================================================
// Computed write values ("установка знаний" via inline `$fn`)
// ============================================================================

/// Detect whether a JSON field value encodes an inline function call
/// (`{ "$fn": ... }`). Such fields are evaluated at write time and replaced
/// by their computed result before the record is interned and persisted.
fn is_computed_field(v: &json::Value) -> bool {
    v.as_object().is_some_and(|o| o.contains_key("$fn"))
}

/// Resolve any inline `$fn` computed fields in a record (the "computed value
/// on write" feature). A field whose value is
/// `{ "$fn": { "name": "strings/lower", "args": [{ "$ref": ["email"] }] } }`
/// is evaluated through the scalar registry; `$ref` arguments resolve against
/// the record's *literal* (non-computed) fields. The computed result replaces
/// the field value.
///
/// **Fail-closed:** any evaluation failure (unknown function, unresolved
/// `$ref`, type / arity error) aborts the write with an `Err` rather than
/// storing a wrong or null value — a computed value is an integrity concern,
/// not a best-effort hint.
///
/// Non-object values and records with no computed fields are returned
/// unchanged, so the common (literal-only) write path pays nothing beyond one
/// `any()` scan.
fn resolve_computed_record(
    value: &json::Value,
    interner: &Interner,
) -> Result<json::Value, String> {
    let obj = match value {
        json::Value::Object(m) => m,
        _ => return Ok(value.clone()),
    };
    if !obj.values().any(is_computed_field) {
        return Ok(value.clone());
    }

    // `$ref` resolves only against literal fields; a reference to another
    // computed field is intentionally unresolved (fail-closed) so computed
    // fields can't depend on evaluation order.
    let literal: json::Map<String, json::Value> = obj
        .iter()
        .filter(|(_, v)| !is_computed_field(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let scalars = builtin_scalars();
    let mut out = obj.clone();
    for (k, v) in obj {
        if !is_computed_field(v) {
            continue;
        }
        let fv: FilterValue =
            serde_json::from_value(v.clone()).map_err(|e| format!("computed field '{k}': {e}"))?;
        let result = eval_write_value(&fv, &literal, interner, scalars)
            .map_err(|e| format!("computed field '{k}': {e}"))?;
        let jv = inner_to_json_value(&result, interner)
            .map_err(|e| format!("computed field '{k}': {e}"))?;
        out.insert(k.clone(), jv);
    }
    Ok(json::Value::Object(out))
}

/// Evaluate a [`FilterValue`] to an [`InnerValue`] in the write-time computed
/// context: literals map directly, `$ref` navigates `literal` (the record's
/// own literal fields), and `$fn` dispatches recursively through the scalar
/// registry.
fn eval_write_value(
    fv: &FilterValue,
    literal: &json::Map<String, json::Value>,
    interner: &Interner,
    scalars: &ScalarRegistry,
) -> Result<InnerValue, String> {
    match fv {
        FilterValue::Null => Ok(InnerValue::Null),
        FilterValue::Bool(b) => Ok(InnerValue::Bool(*b)),
        FilterValue::Int(i) => Ok(InnerValue::Int(*i)),
        FilterValue::Float(f) => Ok(InnerValue::F64(*f)),
        FilterValue::String(s) => Ok(InnerValue::Str(s.clone())),
        FilterValue::Binary(b) => Ok(InnerValue::Bin(b.clone())),
        FilterValue::FieldRef { path } => {
            let leaf = json_nav(literal, path).ok_or_else(|| {
                format!("$ref '{}' not found among literal fields", path.join("."))
            })?;
            json_value_to_inner(leaf, interner).map_err(|e| e.to_string())
        }
        FilterValue::FnCall { call } => {
            let mut args = Vec::with_capacity(call.args().len());
            for a in call.args() {
                args.push(eval_write_value(a, literal, interner, scalars)?);
            }
            scalars
                .call(call.name(), &args)
                .map_err(|e| format!("{}: {}", call.name(), e.code))
        }
        _ => Err("unsupported computed value variant".to_string()),
    }
}

/// Navigate a field path through a JSON object (`["address","zip"]`).
fn json_nav<'a>(
    obj: &'a json::Map<String, json::Value>,
    path: &[String],
) -> Option<&'a json::Value> {
    let mut cur = obj.get(path.first()?)?;
    for seg in &path[1..] {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

/// Build a [`LayeredInterner`] that routes new field names to `tx.interner_overlay`.
fn make_layered_interner<'a>(
    base: &'a shamir_types::core::interner::Interner,
    tx: &'a shamir_tx::TxContext,
) -> shamir_tx::LayeredInterner<'a> {
    shamir_tx::LayeredInterner::Layered {
        base,
        overlay: &tx.interner_overlay,
        next_overlay_id: &tx.next_overlay_id,
    }
}

/// Produce a closure that interns a field name via `layered`, returning
/// an [`InternerKey`](shamir_types::core::interner::InternerKey).
fn intern_via_layered<'a>(
    layered: &'a shamir_tx::LayeredInterner<'a>,
) -> impl Fn(
    &str,
) -> Result<shamir_types::core::interner::InternerKey, shamir_types::codecs::CodecError>
       + 'a {
    move |key: &str| {
        let id = layered.touch_sync(key);
        Ok(shamir_types::core::interner::InternerKey::new(id))
    }
}

impl TableManager {
    /// Execute an INSERT operation.
    ///
    /// Converts each JSON value to InnerValue, inserts into the table,
    /// and returns the inserted records with their generated IDs.
    pub async fn execute_insert(&self, op: &InsertOp) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;

        // 1. Resolve inline `$fn` computed fields first (fail-closed); the
        //    resolved record is what we both store and echo back, so the
        //    client never sees the unevaluated marker. Then convert all JSON
        //    values to InnerValue upfront — any codec error fails the whole
        //    insert with nothing written (the first bad value aborts).
        let mut resolved_values: Vec<json::Value> = Vec::with_capacity(op.values.len());
        for value in &op.values {
            resolved_values.push(
                resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?,
            );
        }
        let mut inner_values: Vec<InnerValue> = Vec::with_capacity(resolved_values.len());
        for value in &resolved_values {
            let inner = json_value_to_inner(value, interner)
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
            inner_values.push(inner);
        }

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
        let mut records = Vec::with_capacity(resolved_values.len());
        for (value, id) in resolved_values.iter().zip(ids.iter()) {
            let mut obj = match value {
                json::Value::Object(map) => map.clone(),
                _ => json::Map::new(),
            };
            obj.insert("_id".to_string(), json::Value::String(id.to_string()));
            records.push(json::Value::Object(obj));
        }

        // Persist any newly interned keys.
        self.flush_metadata().await?;

        // Changefeed: emit one Put per inserted record. The event carries
        // the same MVCC version the data was written at (batch_version).
        let changes: Vec<_> = ids
            .iter()
            .zip(inner_values.iter())
            .filter_map(|(id, iv)| self.put_change(*id, iv))
            .collect();
        self.emit_nontx_changefeed(batch_version, changes).await;

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

        // Resolve inline `$fn` computed fields first (fail-closed), then
        // intern field names through the tx overlay.
        let mut resolved_values: Vec<json::Value> = Vec::with_capacity(op.values.len());
        for value in &op.values {
            resolved_values.push(
                resolve_computed_record(value, interner)
                    .map_err(shamir_storage::error::DbError::Codec)?,
            );
        }
        let inner_values: Vec<InnerValue> = {
            let layered = make_layered_interner(interner, tx);
            let intern_fn = intern_via_layered(&layered);
            let mut vals = Vec::with_capacity(resolved_values.len());
            for value in &resolved_values {
                let inner = json_value_to_inner_with(value, &intern_fn)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                vals.push(inner);
            }
            vals
        };

        // S3: run validators on each record before staging.
        // TODO actor threading — use Actor::System for now.
        for iv in &inner_values {
            self.run_validators(WriteOp::Insert, Some(iv), None, &Actor::System)
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

        let mut records = Vec::with_capacity(resolved_values.len());
        for (value, id) in resolved_values.iter().zip(ids.iter()) {
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

        // Resolve inline `$fn` computed fields, then convert set fields to
        // InnerValue map entries. `$ref` resolves against the other literal
        // fields in the same `set` payload.
        let resolved_set = resolve_computed_record(&op.set, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        let set_inner = json_value_to_inner(&resolved_set, interner)
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
        // Changefeed (Phase 3b follow-up): collect (id, new_value) for each
        // changed record so a Put can be emitted after the batch is durable.
        let mut changefeed_puts: Vec<(RecordId, InnerValue)> = Vec::new();
        // Track the maximum MVCC version across per-record writes so the
        // changefeed event carries the batch's commit-version.
        let mut max_write_version: u64 = 0;
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
                // S3: run validators before persisting.
                // TODO actor threading — use Actor::System for now.
                self.run_validators(
                    WriteOp::Update,
                    Some(&new_record),
                    Some(old_record),
                    &Actor::System,
                )
                .await
                .map_err(validator_failure_to_db_error)?;

                let (_created, ver) = self.set_returning_version(*id, &new_record).await?;
                max_write_version = max_write_version.max(ver);
                affected += 1;
                changefeed_puts.push((*id, new_record.clone()));
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
            self.flush_metadata().await?;
        }

        // Changefeed: emit one Put per changed record. The event carries
        // the max MVCC version from the per-record writes (best-effort).
        let changes: Vec<_> = changefeed_puts
            .iter()
            .filter_map(|(id, iv)| self.put_change(*id, iv))
            .collect();
        self.emit_nontx_changefeed(max_write_version, changes).await;

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
            json_value_to_inner_with(&resolved_set, &intern_fn)
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
        let mut result_records: Vec<json::Value> = Vec::new();
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
                // S3: run validators before staging.
                // TODO actor threading — use Actor::System for now.
                self.run_validators(
                    WriteOp::Update,
                    Some(&new_record),
                    Some(old_record),
                    &Actor::System,
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
                    result_records.push(inner_to_json_value(&new_record, interner)?);
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
        let mut deleted_ids: Vec<RecordId> = Vec::new();
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
        self.emit_nontx_changefeed(max_write_version, changes).await;

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
                self.run_validators(WriteOp::Delete, None, Some(rec), &Actor::System)
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

        // Resolve inline `$fn` computed fields, then convert the new value.
        let resolved_value = resolve_computed_record(&op.value, interner)
            .map_err(shamir_storage::error::DbError::Codec)?;
        let new_inner = json_value_to_inner(&resolved_value, interner)
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
        self.emit_nontx_changefeed(write_version, changes).await;

        Ok(WriteResult {
            affected: 1,
            records: vec![json::Value::Object(result_obj)],
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
                json::Value::Object(map) => {
                    let mut fields = Vec::with_capacity(map.len());
                    for (k, v) in map {
                        let key_id = layered.touch_sync(k.as_str());
                        let inner_v = json_value_to_inner_with(v, &intern_fn)
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

            let new_inner = json_value_to_inner_with(&resolved_value, &intern_fn)
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
            self.run_validators(
                WriteOp::Upsert,
                Some(&merged),
                Some(&existing),
                &Actor::System,
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
            if let json::Value::Object(overlay) = &op.value {
                for (k, v) in overlay {
                    base_obj.insert(k.clone(), v.clone());
                }
            }
            (false, json::Value::Object(base_obj))
        } else {
            // S3: run validators (Upsert — no existing → insert path, tx).
            // TODO actor threading — use Actor::System for now.
            self.run_validators(WriteOp::Upsert, Some(&new_inner), None, &Actor::System)
                .await
                .map_err(validator_failure_to_db_error)?;

            let id = self.insert_tx(&new_inner, Some(&mut *tx)).await?;
            // Build result JSON from original op.value to avoid overlay-id
            // reverse-lookup (overlay ids are not yet in the base interner).
            let mut obj = match op.value.clone() {
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

        // NOTE (audit 3c-C2): stale index entries (id present in index but
        // absent from data_store) are silently skipped via `let … else {
        // continue }`. The old per-`get` loop propagated NotFound as an
        // error; now a stale pointer degrades gracefully. This path is not
        // cheaply unit-reachable without manually corrupting the info_store
        // behind the IndexManager's back (requires raw Store + IndexManager
        // cross-layer setup heavier than justified for a guard-scope fix).
        let mut result = Vec::with_capacity(record_ids.len());
        let id_vec: Vec<RecordId> = record_ids.into_iter().collect();
        // FINAL-A: use the seam-level get_many (reads from the log when
        // an MvccStore is attached) instead of the raw data_store path.
        let records = self.get_many(&id_vec).await?;
        for (id, record_opt) in id_vec.into_iter().zip(records) {
            let Some(record) = record_opt else { continue };
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
