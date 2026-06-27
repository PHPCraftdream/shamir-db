//! Write-path helpers for TableManager.
//!
//! Contains free helper functions used during write execution
//! (computed-field resolution, interning utilities) and the
//! index-backed lookup helpers used by execute_update / execute_delete / execute_set.

use std::borrow::Cow;
use std::collections::BTreeSet;

use futures::StreamExt;

use crate::function::builtin_scalars;
use crate::query::filter::eval::{compile_filter, FilterNode};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use shamir_funclib::registry::ScalarRegistry;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::scalar_ref_cmp;
use shamir_types::record_view::RecordRef;
use shamir_types::types::common::TMap;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::validator::{TransformSpec, ValidatorFailure};

use super::table_manager::TableManager;

/// Convert a [`ValidatorFailure`] into a [`DbError`](shamir_storage::error::DbError).
pub(super) fn validator_failure_to_db_error(
    failure: ValidatorFailure,
) -> shamir_storage::error::DbError {
    match failure {
        ValidatorFailure::Failed(errors) => {
            // Build a typed text summary of field-bound errors without
            // external serialisation. Each entry is rendered as
            // "field.path: code" (record-level errors omit the path).
            // The resulting string always contains every field path and
            // code, so callers that do substring-contains checks on the
            // error message (e.g. `msg.contains("stale")`) continue to
            // work unchanged.
            let msg = errors
                .iter()
                .map(|e| match &e.field {
                    Some(path) => format!("{}: {}", path.join("."), e.code),
                    None => e.code.clone(),
                })
                .collect::<Vec<_>>()
                .join("; ");
            shamir_storage::error::DbError::ValidatorRejected(msg)
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

/// Detect whether a QueryValue field value encodes an inline function call
/// (`{ "$fn": ... }`). Such fields are evaluated at write time and replaced
/// by their computed result before the record is interned and persisted.
pub(super) fn is_computed_field(v: &QueryValue) -> bool {
    match v {
        QueryValue::Map(m) => m.contains_key("$fn"),
        _ => false,
    }
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
pub(super) fn resolve_computed_record<'a>(
    value: &'a QueryValue,
    _interner: &Interner,
) -> Result<Cow<'a, QueryValue>, String> {
    let obj = match value {
        QueryValue::Map(m) => m,
        _ => return Ok(Cow::Borrowed(value)),
    };
    if !obj.values().any(is_computed_field) {
        return Ok(Cow::Borrowed(value));
    }

    // `$ref` resolves only against literal fields; a reference to another
    // computed field is intentionally unresolved (fail-closed) so computed
    // fields can't depend on evaluation order.
    let literal: TMap<String, QueryValue> = obj
        .iter()
        .filter(|(_, v)| !is_computed_field(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let scalars = builtin_scalars();
    let mut out: TMap<String, QueryValue> = obj.clone();
    for (k, v) in obj {
        if !is_computed_field(v) {
            continue;
        }
        // Deserialize the computed field value as FilterValue via the
        // msgpack round-trip: QueryValue → rmp_serde bytes → FilterValue.
        // Both types share the same serde shape (untagged enum), so the
        // msgpack encoding produced by QueryValue is byte-identical to
        // what FilterValue's deserializer expects — no extra encoding involved.
        let bytes = rmp_serde::to_vec_named(v)
            .map_err(|e| format!("computed field '{k}': msgpack encode: {e}"))?;
        let fv: FilterValue =
            rmp_serde::from_slice(&bytes).map_err(|e| format!("computed field '{k}': {e}"))?;
        // C6 (#80): eval_write_value now returns QueryValue directly — the
        // $fn round-trip (inner→query→funclib→query→inner) is gone, and the
        // result flows straight into the (already QueryValue) record map.
        let result = eval_write_value(&fv, &literal, scalars)
            .map_err(|e| format!("computed field '{k}': {e}"))?;
        out.insert(k.clone(), result);
    }
    Ok(Cow::Owned(QueryValue::Map(out)))
}

// ============================================================================
// Literal DEFAULT stamp (Phase ②.4c — insert path)
// ============================================================================

/// Stamp literal default values onto ABSENT fields of a record (Phase ②.4c).
///
/// For each `(path, default)` rule:
/// - **MVP scope: single-segment paths only.** Multi-segment paths
///   (`["address","zip"]`) are silently skipped (future work — nested
///   default stamping needs a recursive walker; none of the current ②.4b
///   surface exercises nested defaults).
/// - **ABSENCE = the field key is NOT present in the record map.** A field
///   explicitly present as `Null` is NOT absent — it is an explicit value
///   and is never overwritten (the keystone replay-safe invariant from
///   DDL-EVOLUTION-PLAN §②.4a variant B).
/// - If absent, `field` is inserted with `default.clone()`. The stamp is
///   therefore idempotent: re-applying on an already-stamped record is a
///   no-op (the key is now present).
///
/// Non-map records are returned unchanged (defaults are field-scoped). The
/// caller is expected to fast-skip when `defaults` is empty (the common hot
/// path — tables without a `default` rule).
pub(super) fn apply_defaults(rec: &mut QueryValue, defaults: &[(Vec<String>, QueryValue)]) {
    let m = match rec {
        QueryValue::Map(m) => m,
        _ => return,
    };
    for (path, default) in defaults {
        // MVP: single-segment path only. A multi-segment default is a
        // future enhancement; skip it here rather than partially applying.
        let field = match path.first() {
            Some(f) if path.len() == 1 => f,
            _ => continue,
        };
        // ABSENCE check: only stamp when the key is missing. Present-as-Null
        // is an explicit value — do NOT overwrite.
        if !m.contains_key(field) {
            m.insert(field.clone(), default.clone());
        }
    }
}

// ============================================================================
// Declarative TRANSFORM stamp (Phase ③.2b — insert path)
// ============================================================================

/// Apply declarative transform rules to a record BEFORE encode (③.2b).
///
/// For each `(path, spec)` rule:
/// - **MVP scope: single-segment paths only.** Multi-segment paths are
///   silently skipped (matching `apply_defaults` MVP — future work).
/// - [`TransformSpec::AutoNow`] — unconditionally overwrites the field with
///   `QueryValue::Int(now_ns as i64)`.  This is the `updated_at` semantic:
///   the server clock is always authoritative.
/// - [`TransformSpec::AutoNowAdd`] — stamps `QueryValue::Int(now_ns as i64)`
///   only if the field is absent (`!contains_key`).  This is the `created_at`
///   semantic: an explicitly-supplied value is preserved.
/// - [`TransformSpec::ComputedDefault`] — evaluates the expression through
///   `eval_write_value` only if the field is absent.  On evaluation error the
///   stamp is skipped silently (fail-open), consistent with the scalar-bridge
///   fail-open precedent from Phase B.  The record map at the time of
///   evaluation is passed as the `literal` context so `$ref` can address
///   sibling fields already stamped by `apply_defaults`.
///
/// Non-map records are returned unchanged (transforms are field-scoped).
/// The caller is expected to fast-skip when `transforms` is empty (the
/// common hot path — no transforms declared).
pub(crate) fn apply_transforms(
    rec: &mut QueryValue,
    transforms: &[(Vec<String>, TransformSpec)],
    scalars: &ScalarRegistry,
    now_ns: u64,
) {
    let m = match rec {
        QueryValue::Map(m) => m,
        _ => return,
    };
    for (path, spec) in transforms {
        // MVP: single-segment path only.  Multi-segment is a future enhancement.
        let field = match path.first() {
            Some(f) if path.len() == 1 => f,
            _ => continue,
        };
        match spec {
            TransformSpec::AutoNow => {
                // Unconditional: overwrites any caller-supplied value.
                // Server clock is authoritative for `updated_at`.
                m.insert(field.clone(), QueryValue::Int(now_ns as i64));
            }
            TransformSpec::AutoNowAdd => {
                // Absence-guarded: preserve an explicitly-supplied value.
                if !m.contains_key(field.as_str()) {
                    m.insert(field.clone(), QueryValue::Int(now_ns as i64));
                }
            }
            TransformSpec::ComputedDefault(expr) => {
                // Absence-guarded: only fill missing fields.
                if !m.contains_key(field.as_str()) {
                    // Snapshot the current map as the literal context for
                    // $ref resolution.  Cloning here is bounded by the number
                    // of ComputedDefault transforms (rare at schema-level) and
                    // avoids a self-borrow conflict between `m` (mut) and the
                    // literal slice passed to eval_write_value.
                    let literal: TMap<String, QueryValue> = m.clone();
                    // Fail-open on error: skip the stamp silently rather than
                    // aborting the write.  This matches the scalar-bridge
                    // fail-open precedent (Phase B, ValidatorCtx::scalars =
                    // None → skip silently).  A future strict mode could
                    // surface the error as a ValidatorFailure, but
                    // ComputedDefault is a best-effort default, not a hard
                    // integrity constraint — that role belongs to CHECK rules.
                    if let Ok(v) = eval_write_value(expr, &literal, scalars) {
                        m.insert(field.clone(), v);
                    }
                }
            }
        }
    }
}

/// Evaluate a [`FilterValue`] to a [`QueryValue`] in the write-time computed
/// context: literals map directly, `$ref` navigates `literal` (the record's
/// own literal fields) as a `QueryValue::Map` path,
/// and `$fn` dispatches recursively through the scalar registry.
///
/// C6 (#80): the `$fn` branch builds QueryValue args and keeps the funclib
/// result as QueryValue — the previous `inner→query→funclib→query→inner`
/// round-trip (old lines 152/159) is retired. The result flows straight
/// into the record map (already QueryValue per M2).
pub(super) fn eval_write_value(
    fv: &FilterValue,
    literal: &TMap<String, QueryValue>,
    scalars: &ScalarRegistry,
) -> Result<QueryValue, String> {
    match fv {
        FilterValue::Null => Ok(QueryValue::Null),
        FilterValue::Bool(b) => Ok(QueryValue::Bool(*b)),
        FilterValue::Int(i) => Ok(QueryValue::Int(*i)),
        FilterValue::Float(f) => Ok(QueryValue::F64(*f)),
        FilterValue::String(s) => Ok(QueryValue::Str(s.clone())),
        FilterValue::Binary(b) => Ok(QueryValue::Bin(b.clone())),
        FilterValue::FieldRef { path } => qv_nav(literal, path)
            .cloned()
            .ok_or_else(|| format!("$ref '{}' not found among literal fields", path.join("."))),
        FilterValue::FnCall { call } => {
            // Args are QueryValue → straight to funclib; result kept as
            // QueryValue. Zero InnerValue, zero round-trip (C6 #80).
            let mut qv_args = Vec::with_capacity(call.args().len());
            for a in call.args() {
                let qv = eval_write_value(a, literal, scalars)?;
                qv_args.push(qv);
            }
            scalars
                .call(call.name(), &qv_args)
                .map_err(|e| format!("{}: {}", call.name(), e.code))
        }
        _ => Err("unsupported computed value variant".to_string()),
    }
}

/// Navigate a field path through a `QueryValue::Map` (`["address", "zip"]`).
///
/// Replaces the former `qv_nav` over a QueryValue map. The top-level map
/// is a `TMap<String, QueryValue>`; nested maps are `QueryValue::Map`.
pub(super) fn qv_nav<'a>(
    obj: &'a TMap<String, QueryValue>,
    path: &[String],
) -> Option<&'a QueryValue> {
    let first = path.first()?;
    let mut cur: &QueryValue = obj.get(first.as_str())?;
    for seg in &path[1..] {
        match cur {
            QueryValue::Map(m) => cur = m.get(seg.as_str())?,
            _ => return None,
        }
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
            for (id, cow) in batch {
                let record = cow.into_inner()?;
                let all_match = key_fields.iter().all(|(path, expected)| {
                    // Stage 3: go through RecordRef::scalar_at + scalar_ref_cmp
                    // instead of resolve_field + compare_values. The path is
                    // &[u64]; scalar_at takes &[InternerKey]. Convert on the
                    // stack (paths are 1-3 segments).
                    let ipath: smallvec::SmallVec<[InternerKey; 4]> =
                        path.iter().map(|&id| InternerKey::new(id)).collect();
                    record
                        .scalar_at(&ipath)
                        .and_then(|s| scalar_ref_cmp(s, expected))
                        == Some(std::cmp::Ordering::Equal)
                });
                if all_match {
                    return Ok(Some((id, record)));
                }
            }
        }
        Ok(None)
    }
}
