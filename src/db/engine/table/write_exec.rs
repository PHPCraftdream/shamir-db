//! Write operation execution on TableManager.
//!
//! Implements execute_insert, execute_update, execute_delete for TableManager.

use std::time::Instant;

use futures::StreamExt;
use serde_json as json;

use crate::codecs::interned::{inner_to_json_value, json_value_to_inner};
use crate::core::interner::InternerKey;
use crate::db::query::filter::eval::compile_filter;
use crate::db::query::filter::eval_context::FilterContext;
use crate::db::query::write::{DeleteOp, InsertOp, UpdateOp, UpdateReturnMode, WriteResult};
use crate::db::DbResult;
use crate::types::value::InnerValue;

use super::table_manager::TableManager;

impl TableManager {
    /// Execute an INSERT operation.
    ///
    /// Converts each JSON value to InnerValue, inserts into the table,
    /// and returns the inserted records with their generated IDs.
    pub async fn execute_insert(&self, op: &InsertOp) -> DbResult<WriteResult> {
        let start = Instant::now();
        let interner = self.interner().get().await?;
        let mut records = Vec::with_capacity(op.values.len());

        for value in &op.values {
            let inner = json_value_to_inner(value, interner)
                .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;

            let id = self.insert(&inner).await?;

            // Build result record: original fields + _id
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
            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
        let set_map = match &set_inner {
            InnerValue::Map(m) => m,
            _ => {
                return Err(crate::db::DbError::Validation(
                    "UPDATE set must produce a Map".to_string(),
                ))
            }
        };

        // Collect matching records (need to read-then-write)
        let matched = if let Some(ref filter) = op.where_clause {
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
        } else {
            // No where clause — update ALL records
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
                    result_records.push(inner_to_json_value(&new_record, interner));
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

        let callback = compile_filter(&op.where_clause, interner);

        // Collect IDs to delete (can't delete while streaming)
        let mut to_delete = Vec::new();
        let stream = self.list_stream(batch_size);
        futures::pin_mut!(stream);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (id, record) in batch {
                if callback.matches(&record, ctx) {
                    to_delete.push(id);
                }
            }
        }

        let mut affected: u64 = 0;
        for id in to_delete {
            if self.delete(id).await? {
                affected += 1;
            }
        }

        Ok(WriteResult {
            affected,
            records: Vec::new(),
            execution_time_us: start.elapsed().as_micros() as u64,
        })
    }
}

/// Merge set_map fields into an existing InnerValue record.
///
/// Only works on Map values. For each key in set_map, overwrite
/// the corresponding key in the original. Keys not in set_map
/// are preserved.
fn merge_inner_maps(
    original: &InnerValue,
    set_map: &crate::types::common::TMap<InternerKey, InnerValue>,
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
