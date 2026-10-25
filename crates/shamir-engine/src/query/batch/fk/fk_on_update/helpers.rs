use crate::table::record_cow::RecordCow;

use super::plan::PendingMutation;

use bytes::Bytes;
use std::collections::BTreeSet;

use shamir_query_types::admin::FkAction;
use shamir_types::record_view::{scalar_ref_numeric_eq, RecordRef, RecordView, ScalarRef};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::{FilterContext, FilterNode};
use crate::table::TableManager;

// ── Helpers ──────────────────────────────────────────────────────────────────

pub(super) fn cow_to_bytes(cow: RecordCow) -> shamir_storage::error::DbResult<Bytes> {
    match cow {
        RecordCow::Borrowed(b) => Ok(b),
        RecordCow::Owned(tree) => tree.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("fk_on_update scan serialize: {e}"))
        }),
    }
}

pub(super) fn filter_matches(
    bytes: &Bytes,
    callback: &FilterNode,
    ctx: &FilterContext<'_>,
) -> bool {
    match RecordView::new(bytes) {
        Ok(view) => callback.matches(&view, ctx),
        Err(_) => {
            if let Ok(tree) = InnerValue::from_bytes(bytes.clone()) {
                callback.matches(&tree, ctx)
            } else {
                false
            }
        }
    }
}

pub(super) fn scalar_ref_matches_qv(actual: &ScalarRef<'_>, value: &QueryValue) -> bool {
    // Delegate to the EXACT numeric equality predicate (`scalar_ref_numeric_eq`,
    // RV-FUNCLIB-5FA455F7 S2) so that a parent key stored as `Int(5)` matches a
    // child FK field stored as `F64(5.0)` (and vice-versa) while Int↔Big stays
    // exact beyond 2^53 and NaN matches nothing.
    scalar_ref_numeric_eq(*actual, value)
}

/// Keyed by an already-resolved `InternerKey` instead of a field name — the
/// field id must come from `resolve_field_id_layered` (base + this tx's
/// overlay), not a plain by-name interner lookup, since the matched row may
/// be tx-overlay-sourced (`list_stream_tx`) and use an overlay-only field id.
pub(super) fn record_field_matches_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    value: &QueryValue,
) -> bool {
    let path = std::slice::from_ref(field_id);
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return scalar_ref_matches_qv(&actual, value);
        }
    }
    false
}

/// Like [`record_field_matches_by_id`], but tests a SINGLE decoded row
/// against every member of `values` — the row is decoded exactly once
/// regardless of how many values are probed, closing the redundant
/// per-probe re-decode `any_child_references`'s old per-value shape caused.
pub(super) fn record_field_matches_any_by_id(
    record_bytes: &[u8],
    field_id: &shamir_types::core::interner::InternerKey,
    values: &[&QueryValue],
) -> bool {
    let path = std::slice::from_ref(field_id);
    if let Ok(view) = RecordView::new(record_bytes) {
        if let Some(actual) = view.scalar_at(path) {
            return values.iter().any(|v| scalar_ref_matches_qv(&actual, v));
        }
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(record_bytes)) {
        if let Some(actual) = tree.scalar_at(path) {
            return values.iter().any(|v| scalar_ref_matches_qv(&actual, v));
        }
    }
    false
}

/// Row decoded once for testing against multiple probes — see `decode_row`.
enum DecodedRow<'a> {
    View(RecordView<'a>),
    Tree(InnerValue),
}

impl DecodedRow<'_> {
    fn scalar_at(
        &self,
        path: &[shamir_types::core::interner::InternerKey],
    ) -> Option<ScalarRef<'_>> {
        match self {
            DecodedRow::View(v) => v.scalar_at(path),
            DecodedRow::Tree(t) => t.scalar_at(path),
        }
    }
}

/// Decode `bytes` once via `RecordView`, falling back to `InnerValue` for
/// non-map records — shared across every probe tested against the SAME row
/// (closes the redundant per-probe re-decode `record_field_matches_by_id`
/// caused when invoked once per probe for one row in `classify_row_update`).
fn decode_row(bytes: &[u8]) -> Option<DecodedRow<'_>> {
    if let Ok(view) = RecordView::new(bytes) {
        return Some(DecodedRow::View(view));
    }
    if let Ok(tree) = InnerValue::from_bytes(Bytes::copy_from_slice(bytes)) {
        return Some(DecodedRow::Tree(tree));
    }
    None
}

/// F-53c: per-row fan-out resolution shared by the index fast-path and the
/// `list_stream_tx` scan fallback in `plan_fk_on_update`. Mutates `mutations`
/// exactly as the original inline loop did: the UPDATE path is PER-FIELD (a
/// single row may carry two distinct FK fields that both reference the old
/// value, and each must be re-keyed or nulled according to its own FK's
/// action). Extracting this into a shared helper guarantees the two paths
/// produce byte-for-byte identical results whether or not a supporting
/// index exists on the FK column.
pub(super) fn classify_row_update(
    id: RecordId,
    bytes: &[u8],
    resolved_probes: &[(&QueryValue, &QueryValue, FkAction, &str, u64)],
    child_table: &TableManager,
    mutations: &mut Vec<PendingMutation>,
) {
    // Decode the row ONCE regardless of how many probes it's tested
    // against — `record_field_matches_by_id` previously re-decoded these
    // SAME bytes from scratch on every probe iteration below.
    let Some(row) = decode_row(bytes) else {
        // Corrupt row: every probe below would already have failed its own
        // decode identically, so "no match for this row" is the same
        // outcome as before.
        return;
    };
    // For this row, apply EVERY matching probe.  A field is a single FK with
    // a single action, and a row's field value can equal at most one `old`
    // value, so at most one probe per field matches a given row — there is
    // no per-field Cascade/SetNull conflict to resolve.
    for (old, new, action, field, field_id) in resolved_probes {
        let key = shamir_types::core::interner::InternerKey::new(*field_id);
        let path = std::slice::from_ref(&key);
        let matches = row
            .scalar_at(path)
            .is_some_and(|actual| scalar_ref_matches_qv(&actual, old));
        if !matches {
            continue;
        }
        match action {
            FkAction::Cascade => {
                mutations.push(PendingMutation::UpdateField {
                    table: child_table.clone(),
                    id,
                    field: field.to_string(),
                    new_value: (*new).clone(),
                });
            }
            FkAction::SetNull => {
                mutations.push(PendingMutation::SetNull {
                    table: child_table.clone(),
                    id,
                    field: field.to_string(),
                });
            }
            _ => {}
        }
    }
}

/// F-53c: index fast-path candidate gathering, mirroring
/// `fk_restrict::child_has_reference`'s index lookup exactly (and the
/// identical helper in `fk_actions.rs`). See `fk_actions::index_candidate_ids`
/// for the full contract. The caller MUST have already recorded the RI-barrier
/// token at entry (before this call), preserving the F-40b mutual-serialization
/// ordering exactly as `child_has_reference` does.
pub(super) async fn index_candidate_ids(
    table: &TableManager,
    interner: &shamir_types::core::interner::Interner,
    probes: &[(&str, &QueryValue)],
) -> shamir_storage::error::DbResult<Option<Vec<RecordId>>> {
    let mut all: BTreeSet<RecordId> = BTreeSet::new();
    for (field, value) in probes {
        let inner = match qv_scalar_to_inner(value) {
            Some(InnerValue::Null) | None => return Ok(None),
            Some(iv) => iv,
        };
        let base_field_id = match interner.get_ind(field) {
            Some(id) => id,
            None => return Ok(None),
        };
        let field_path = [base_field_id.id()];
        let idx_name = match table.find_single_field_index(&field_path) {
            Some(n) => n,
            None => return Ok(None),
        };
        let ids = match table
            .index_manager_ref()
            .lookup_by_index(idx_name, std::slice::from_ref(&inner))
            .await?
        {
            // P0-3a (#1011): `None` = a DROP of this index is in its drain→sweep
            // window — index momentarily unusable. Bail to a full scan
            // (`Ok(None)`) instead of treating it as "no matches".
            Some(ids) => ids,
            None => return Ok(None),
        };
        all.extend(ids.iter().copied());
    }
    Ok(Some(all.into_iter().collect()))
}

pub(super) fn scalar_ref_to_qv(sr: ScalarRef<'_>) -> QueryValue {
    match sr {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.to_owned()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}

pub(super) fn qv_scalar_to_inner(qv: &QueryValue) -> Option<InnerValue> {
    match qv {
        QueryValue::Null => Some(InnerValue::Null),
        QueryValue::Bool(b) => Some(InnerValue::Bool(*b)),
        QueryValue::Int(i) => Some(InnerValue::Int(*i)),
        QueryValue::F64(f) => Some(InnerValue::F64(*f)),
        QueryValue::Str(s) => Some(InnerValue::Str(s.clone())),
        QueryValue::Bin(b) => Some(InnerValue::Bin(b.clone())),
        QueryValue::Dec(d) => Some(InnerValue::Dec(*d)),
        QueryValue::Big(b) => Some(InnerValue::Big(b.clone())),
        _ => None,
    }
}

/// Convert a query-value scalar to its inner (interner-keyed) form for the
/// storage byte-merge.  Non-scalar types fall back to Null (they are rejected
/// earlier by `qv_is_supported_scalar`).
pub(super) fn qv_to_inner_scalar(qv: &QueryValue) -> InnerValue {
    qv_scalar_to_inner(qv).unwrap_or(InnerValue::Null)
}

/// A `new_value` we can propagate at plan time: a literal scalar.
///
/// `Map` / `List` / `Set` are non-scalar; the `$fn`/`$ref`/`$expr` markers
/// surface here as `Map`/`List` (the reference carrier shape), so this guard
/// rejects them too.
pub(super) fn qv_is_supported_scalar(qv: &QueryValue) -> bool {
    matches!(
        qv,
        QueryValue::Null
            | QueryValue::Bool(_)
            | QueryValue::Int(_)
            | QueryValue::F64(_)
            | QueryValue::Str(_)
            | QueryValue::Bin(_)
            | QueryValue::Dec(_)
            | QueryValue::Big(_)
    )
}

pub(super) fn action_name(a: FkAction) -> &'static str {
    match a {
        FkAction::NoAction => "no_action",
        FkAction::Restrict => "restrict",
        FkAction::Cascade => "cascade",
        FkAction::SetNull => "set_null",
    }
}
