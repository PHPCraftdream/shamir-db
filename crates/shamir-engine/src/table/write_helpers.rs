//! Write-path helpers for TableManager.
//!
//! Contains free helper functions used during write execution
//! (computed-field resolution, interning utilities) and the
//! index-backed lookup helpers used by execute_update / execute_delete / execute_set.

use std::collections::BTreeSet;

use futures::StreamExt;
use serde_json as json;

use crate::function::builtin_scalars;
use crate::query::filter::eval::resolve_field;
use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_funclib::registry::ScalarRegistry;
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::{inner_to_json_value, json_value_to_inner};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::validator::ValidatorFailure;

use super::table_manager::TableManager;

/// Convert a [`ValidatorFailure`] into a [`DbError`](shamir_storage::error::DbError).
pub(super) fn validator_failure_to_db_error(
    failure: ValidatorFailure,
) -> shamir_storage::error::DbError {
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
pub(super) fn is_computed_field(v: &json::Value) -> bool {
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
pub(super) fn resolve_computed_record(
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
pub(super) fn eval_write_value(
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
pub(super) fn json_nav<'a>(
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
pub(super) fn make_layered_interner<'a>(
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
pub(super) fn intern_via_layered<'a>(
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
    pub(super) async fn lookup_records_via_index(
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
    pub(super) async fn lookup_existing_for_set(
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
