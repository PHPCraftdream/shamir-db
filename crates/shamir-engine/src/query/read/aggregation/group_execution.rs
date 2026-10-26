//! GROUP BY / aggregate-all execution entry points.
//!
//! Split out of the former `read/aggregate.rs` (bodies unchanged).

use bytes::Bytes;
use indexmap::map::Entry;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_storage::error::DbResult;
use shamir_types::codecs::interned::inner_value_to_query_value;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{HavingView, RecordRef, RecordView, ScalarRef};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::{compile_filter, FilterContext};
use crate::query::read::exec::pre_intern_select_keys;
use crate::query::read::{GroupBy, QueryResult, Select, SelectItem};

use super::accumulator::{
    group_key_item_complex, group_key_item_scalar, intern_field_path_keys, GroupKeyItem,
};
use super::function_inputs::make_aggregate_fn;
use super::group_projection::{build_aggregate_object, scalar_ref_to_query, RowView};
use super::validation::validate_aggregate_select;

// ============================================================================

/// Apply GROUP BY + aggregation + HAVING.
pub fn apply_group_by(
    records: &[(RecordId, Bytes)],
    group_by: &GroupBy,
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> Vec<QueryValue> {
    try_apply_group_by(records, group_by, select, interner, ctx)
        .expect("apply_group_by: aggregate execution failed")
}

/// Fallible grouped execution; legacy callers retain their Vec-returning helper.
pub(crate) fn try_apply_group_by(
    records: &[(RecordId, Bytes)],
    group_by: &GroupBy,
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> DbResult<Vec<QueryValue>> {
    validate_aggregate_select(select)?;
    // Validate bindings even when there are no groups to build.
    for item in &select.items {
        if let SelectItem::AggregateFn { name, args, .. } = item {
            make_aggregate_fn(name, args, ctx)?;
        }
    }
    if records.is_empty() {
        return Ok(Vec::new());
    }

    // Pre-intern group-by field paths.
    let group_paths: Vec<(String, Option<Vec<InternerKey>>)> = group_by
        .fields
        .iter()
        .map(|f| {
            let display_name = f.last().cloned().unwrap_or_default();
            (display_name, intern_field_path_keys(f, interner))
        })
        .collect();

    // Build groups: typed `Vec<GroupKeyItem>` key drives an `IndexMap`
    // hashed via FxHasher. Each group's key values stay alongside the
    // record list so the output projection can read them without re-hitting
    // the records.
    #[allow(clippy::type_complexity)] // grouped aggregate accumulator; clarity over brevity
    let mut groups: TMap<
        Vec<GroupKeyItem>,
        (Vec<(RecordId, Bytes)>, Vec<(String, QueryValue)>),
    > = new_map_wc(0);

    // Per-execution counter backing GroupKeyItem::NanOnce — advanced once
    // per NaN leaf, so each NaN occurrence is its own group (NaN != NaN).
    let mut nan_counter: u64 = 0;
    for (id, bytes) in records {
        // Per-row lens for the group-key resolution (same hot/fallback pair
        // as the accumulator loop). The bytes are refcounted `Bytes` —
        // cheap to clone into the group bucket.
        let key_items: Vec<GroupKeyItem> = match RecordView::new(bytes) {
            Ok(v) => group_paths
                .iter()
                .map(|(_, path)| {
                    group_key_from_lens(&v, path.as_deref(), interner, &mut nan_counter)
                })
                .collect(),
            Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                Ok(iv) => group_paths
                    .iter()
                    .map(|(_, path)| {
                        group_key_from_lens(&iv, path.as_deref(), interner, &mut nan_counter)
                    })
                    .collect(),
                Err(_) => continue, // malformed — skip defensively
            },
        };

        match groups.entry(key_items) {
            Entry::Occupied(mut e) => {
                e.get_mut().0.push((*id, bytes.clone()));
            }
            Entry::Vacant(v) => {
                // First row of a new group — compute the output key values
                // (QueryValue form, for the projection). Same per-row lens.
                let mut key_qv_values = Vec::with_capacity(group_paths.len());
                let view = match RecordView::new(bytes) {
                    Ok(rv) => RowView::Lens(rv),
                    Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                        Ok(iv) => RowView::Tree(iv),
                        // Unreachable: the key_items arm above already
                        // skipped malformed rows. Keep a safe fallback.
                        Err(_) => RowView::Tree(InnerValue::Null),
                    },
                };
                for (field_name, interned_path) in &group_paths {
                    let qv = view
                        .with_ref(|r| group_value_to_query(r, interned_path.as_deref(), interner));
                    key_qv_values.push((field_name.clone(), qv));
                }
                v.insert((vec![(*id, bytes.clone())], key_qv_values));
            }
        }
    }

    // The previous `BTreeMap<String, _>` ordering produced alphabetical
    // group output; tests depend on that. IndexMap::sort_keys does the
    // same in-place.
    groups.sort_keys();

    let mut result: Vec<QueryValue> = Vec::with_capacity(groups.len());
    for (_k, (recs, key_vals)) in &groups {
        result.push(build_aggregate_object(
            recs,
            select,
            Some(key_vals),
            interner,
            ctx,
        )?);
    }

    // Apply HAVING filter — S4: compare via the `QueryValue` result map
    // directly (no `query_value_to_inner` bridge). `HavingView` is a thin
    // `RecordRef` adapter that re-keys String → `InternerKey` once and
    // serves leaves as `ScalarRef` straight off the `QueryValue`.
    if let Some(having_filter) = &group_by.having {
        // RU8: a HAVING on a new alias at the quota cap must surface the
        // typed error, not silently absorb the failed intern.
        pre_intern_select_keys(select, interner)?;
        let having_cb = compile_filter(having_filter, interner);
        result.retain(|qv| {
            let view = HavingView::new(qv, interner);
            having_cb.matches(&view, ctx)
        });
    }

    Ok(result)
}

/// Resolve a single group-by field to a `GroupKeyItem` from a `RecordRef`
/// lens. Scalar leaves go through the lens directly; container / Dec / Big
/// leaves fall back to one `materialize_at` + canonical string form (rare).
#[inline]
fn group_key_from_lens<R: RecordRef + ?Sized>(
    record: &R,
    path: Option<&[InternerKey]>,
    interner: &Interner,
    nan_counter: &mut u64,
) -> GroupKeyItem {
    let Some(path) = path else {
        return GroupKeyItem::Missing;
    };
    if let Some(s) = record.scalar_at(path) {
        // Per-occurrence NaN (S2): every NaN leaf gets its own
        // occurrence-unique key; `group_key_item_scalar` cannot allocate
        // the id (no counter), so the NaN check happens here.
        if let ScalarRef::F64(f) = s {
            if f.is_nan() {
                *nan_counter += 1;
                return GroupKeyItem::NanOnce(*nan_counter);
            }
        }
        return group_key_item_scalar(Some(s));
    }
    // Container / Dec / Big — materialise this one field only.
    match record.materialize_at(path) {
        Some(v) => group_key_item_complex(&v, interner),
        None => GroupKeyItem::Missing,
    }
}

/// Resolve a single group-by field to its `QueryValue` output form (for the
/// group-key projection). Scalars are built from `ScalarRef` directly;
/// containers materialise once at the boundary.
#[inline]
fn group_value_to_query<R: RecordRef + ?Sized>(
    record: &R,
    path: Option<&[InternerKey]>,
    interner: &Interner,
) -> QueryValue {
    let Some(path) = path else {
        return QueryValue::Null;
    };
    if let Some(s) = record.scalar_at(path) {
        return scalar_ref_to_query(s);
    }
    match record.materialize_at(path) {
        Some(v) => inner_value_to_query_value(&v, interner).unwrap_or(QueryValue::Null),
        None => QueryValue::Null,
    }
}

// ============================================================================
// Aggregate All (no GROUP BY but aggregates in SELECT)
// ============================================================================

/// When SELECT contains aggregates but no GROUP BY — aggregate over the entire set.
pub fn apply_aggregate_all(
    records: &[(RecordId, Bytes)],
    select: &Select,
    interner: &Interner,
    scalars: ScalarResolver,
) -> Vec<QueryValue> {
    let empty_refs: TMap<String, QueryResult> = new_map_wc(0);
    let ctx = FilterContext::new(interner, &empty_refs).with_scalars(scalars);
    apply_aggregate_all_with_context(records, select, interner, &ctx)
        .expect("apply_aggregate_all: aggregate execution failed")
}

/// Aggregate one execution with its resolved references and parameters.
/// The scalar-only wrapper remains compatible with existing callers.
pub(crate) fn apply_aggregate_all_with_context(
    records: &[(RecordId, Bytes)],
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> DbResult<Vec<QueryValue>> {
    validate_aggregate_select(select)?;
    let obj = build_aggregate_object(records, select, None, interner, ctx)?;
    Ok(vec![obj])
}
