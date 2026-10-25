//! Read query execution pipeline.
//!
//! Pipeline without GROUP BY:
//!   WHERE (filter_stream) → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult
//!
//! Pipeline with GROUP BY:
//!   WHERE (filter_stream) → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult

use crate::query::filter::eval_context::FilterContext;
pub use crate::query::read::aggregate::{
    apply_aggregate_all, apply_group_by, validate_aggregate_select,
};
pub(crate) use crate::query::read::aggregate::{
    apply_aggregate_all_with_context, try_apply_group_by,
};
use crate::query::read::hashable_query_value::HashableQueryValue;
pub use crate::query::read::order::{
    apply_order_by_qv, apply_order_by_qv_with_ids, apply_order_by_topk, TopKHeap,
};
pub use crate::query::read::select_projection::SelectProjection;
pub use crate::query::read::{Pagination, PaginationInfo, Select, SelectItem};
use indexmap::IndexSet;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

// ============================================================================
// Select projection (public API)
// ============================================================================

/// Apply SELECT projection to raw records, producing QueryValues.
///
/// QueryValue-native projection. Aggregate items are skipped (handled by the
/// aggregate pipeline).
///
/// Without an execution context, external query references and parameters are
/// rejected before projection. Missing fields and scalar Null fallbacks remain
/// unchanged. Aggregate items are still handled by the aggregate pipeline.
pub fn apply_select_value(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
    scalars: ScalarResolver,
) -> DbResult<Vec<QueryValue>> {
    if select.items.iter().any(|item| match item {
        SelectItem::Function { args, .. } => args.iter().any(value_requires_bindings),
        SelectItem::Expression { expr, .. } => value_requires_bindings(&expr.to_filter_value()),
        _ => false,
    }) {
        return Err(DbError::Validation(
            "select_bindings_require_context".into(),
        ));
    }
    let refs = shamir_types::types::common::new_map();
    let ctx = FilterContext::new(interner, &refs).with_scalars(scalars);
    apply_select_value_with_context(records, select, interner, &ctx)
}

/// Inspect syntax rather than evaluated values: Null is a valid field/scalar
/// result, and even an unselected conditional branch can require bindings.
fn value_requires_bindings(value: &FilterValue) -> bool {
    match value {
        FilterValue::QueryRef { .. } | FilterValue::Param { .. } => true,
        FilterValue::Array(values) => values.iter().any(value_requires_bindings),
        FilterValue::Expr { expr } => expr.args.iter().any(value_requires_bindings),
        FilterValue::FnCall { call } => call.args().iter().any(value_requires_bindings),
        FilterValue::Cond { cond } => {
            filter_requires_bindings(&cond.condition)
                || value_requires_bindings(&cond.then)
                || value_requires_bindings(&cond.or_else)
        }
        FilterValue::Null
        | FilterValue::Bool(_)
        | FilterValue::Int(_)
        | FilterValue::Float(_)
        | FilterValue::Binary(_)
        | FilterValue::String(_)
        | FilterValue::FieldRef { .. } => false,
    }
}

fn filter_requires_bindings(filter: &Filter) -> bool {
    match filter {
        Filter::Eq { value, .. }
        | Filter::Ne { value, .. }
        | Filter::Gt { value, .. }
        | Filter::Gte { value, .. }
        | Filter::Lt { value, .. }
        | Filter::Lte { value, .. }
        | Filter::Contains { value, .. }
        | Filter::FieldEq { value, .. } => value_requires_bindings(value),
        Filter::In { values, .. }
        | Filter::NotIn { values, .. }
        | Filter::ContainsAny { values, .. }
        | Filter::ContainsAll { values, .. } => values.iter().any(value_requires_bindings),
        Filter::Between { from, to, .. } => {
            value_requires_bindings(from) || value_requires_bindings(to)
        }
        Filter::ValueCompare { left, right, .. } => {
            value_requires_bindings(left) || value_requires_bindings(right)
        }
        Filter::And { filters } | Filter::Or { filters } => {
            filters.iter().any(filter_requires_bindings)
        }
        Filter::Not { filter } => filter_requires_bindings(filter),
        Filter::Computed {
            expr_args, value, ..
        } => {
            value_requires_bindings(value)
                || expr_args
                    .as_ref()
                    .is_some_and(|args| args.iter().any(value_requires_bindings))
        }
        Filter::Like { .. }
        | Filter::ILike { .. }
        | Filter::Regex { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. }
        | Filter::Fts { .. }
        | Filter::VectorSimilarity { .. } => false,
    }
}

/// Apply SELECT with the read's execution-local references and parameters.
pub(crate) fn apply_select_value_with_context(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> DbResult<Vec<QueryValue>> {
    let proj = SelectProjection::new(select, interner, ctx.scalars.clone())?.with_context(ctx);
    Ok(records
        .iter()
        .map(|(_, record)| proj.project_value(record, interner))
        .collect())
}

// ============================================================================
// Aggregation helpers
// ============================================================================

/// Check whether the select list contains any aggregates.
pub fn has_aggregates(select: &Select) -> bool {
    select.items.iter().any(|item| {
        matches!(
            item,
            SelectItem::Aggregate { .. }
                | SelectItem::CountAll { .. }
                | SelectItem::AggregateFn { .. }
        )
    })
}

/// Pre-intern all output key names from the Select items into the interner.
///
/// This ensures that `compile_filter` (which uses `intern_field_path` / `get_ind`)
/// can resolve field paths that refer to aggregate output keys like "total_age".
pub(crate) fn pre_intern_select_keys(select: &Select, interner: &Interner) {
    for item in &select.items {
        let key = match item {
            SelectItem::Field { path, alias } => {
                if let Some(a) = alias {
                    a.as_str()
                } else if let Some(last) = path.last() {
                    last.as_str()
                } else {
                    continue;
                }
            }
            SelectItem::CountAll { alias } => alias.as_deref().unwrap_or("count"),
            SelectItem::Aggregate { alias, .. } => {
                if let Some(a) = alias {
                    a.as_str()
                } else {
                    continue;
                }
            }
            SelectItem::AggregateFn { alias, .. } => {
                if let Some(a) = alias {
                    a.as_str()
                } else {
                    continue;
                }
            }
            SelectItem::Function { alias, name, .. } => alias.as_deref().unwrap_or(name.as_str()),
            _ => continue,
        };
        // touch_ind ensures the key is interned (creates if missing)
        let _ = interner.touch_ind(key);
    }
}

// ============================================================================
// Pagination
// ============================================================================

/// Pagination metadata for a LIMIT "fast path" — the in-memory top-K heap
/// (`read_collecting`) and the sorted-index walk (`read_order_limit_fast`).
/// These paths apply a finite LIMIT without computing a total count, so the
/// metadata mirrors [`apply_pagination`] with `count_total == false` (total
/// is `None`).
///
/// **Every** LIMIT fast path MUST route its pagination through this single
/// helper. Returning a bare `None` is exactly the #128 regression that
/// silently dropped pagination on two independent fast paths — funnelling
/// them through one function makes that drift impossible to reintroduce
/// without tripping the `limit_queries_all_emit_pagination_contract` test.
pub fn fast_path_pagination(pagination: &Pagination) -> Option<PaginationInfo> {
    if pagination.is_none() {
        None
    } else {
        Some(PaginationInfo::compute(pagination, None))
    }
}

/// Pagination metadata for the inline top-K heap path (F-53a, #874).
///
/// The heap itself only ever holds `k = skip + take` rows, so it cannot
/// supply a true total — but the scan loop that feeds it DOES see every
/// WHERE-passing row, and counts them in an independent running counter.
/// `total_count` is that counter; `count_total` selects whether to thread it
/// into the returned metadata.
///
/// This mirrors [`apply_pagination`]'s metadata logic byte-for-byte (the
/// `pagination.is_none() && count_total` special shape, plus
/// [`PaginationInfo::compute`] with the optional total) — only the SLICING is
/// absent, because the heap already applied the `skip`/`take` window. Keeping
/// the metadata shape identical means a top-K-served `ORDER BY + LIMIT` query
/// returns the same `PaginationInfo` whether or not the bounded heap was used.
pub fn topk_pagination(
    pagination: &Pagination,
    count_total: bool,
    total_count: u64,
) -> Option<PaginationInfo> {
    if pagination.is_none() && !count_total {
        return None;
    }
    let total = if count_total { Some(total_count) } else { None };
    if pagination.is_none() && count_total {
        // Mirrors apply_pagination's "count_total without pagination" shape.
        Some(PaginationInfo {
            total_count: total,
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        })
    } else {
        Some(PaginationInfo::compute(pagination, total))
    }
}

/// Apply pagination to results, returning (page_records, pagination_info).
pub fn apply_pagination<T>(
    records: Vec<T>,
    pagination: &Pagination,
    count_total: bool,
) -> (Vec<T>, Option<PaginationInfo>) {
    if pagination.is_none() && !count_total {
        return (records, None);
    }

    let total = if count_total {
        Some(records.len() as u64)
    } else {
        None
    };

    let (skip, take) = pagination.resolve();
    let skip = skip as usize;

    let sliced: Vec<T> = {
        let mut v = records;
        if skip > 0 {
            let tail = v.split_off(skip.min(v.len()));
            v = tail;
        }
        if let Some(limit) = take {
            v.truncate(limit as usize);
        }
        v
    };

    // Determine has_next
    let mut info = PaginationInfo::compute(pagination, total);
    if total.is_none() {
        // Without total count we can't determine has_next from PaginationInfo::compute,
        // but if we know the original length we can hint
        // (this is already handled by total being None)
    }

    if pagination.is_none() && count_total {
        // Only count_total requested, no actual pagination
        info = PaginationInfo {
            total_count: total,
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        };
    }

    (sliced, Some(info))
}

// ============================================================================
// Distinct
// ============================================================================

/// Does `qv` contain a NaN leaf at ANY depth (including top level)?
fn contains_nan_leaf(qv: &QueryValue) -> bool {
    match qv {
        QueryValue::F64(f) => f.is_nan(),
        QueryValue::List(l) => l.iter().any(contains_nan_leaf),
        QueryValue::Set(s) => s.iter().any(contains_nan_leaf),
        QueryValue::Map(m) => m.iter().any(|(_, v)| contains_nan_leaf(v)),
        _ => false,
    }
}

/// Replace every NaN leaf (at any depth) with a NaN whose payload bits
/// encode a globally-unique occurrence id — each NaN becomes its own raw
/// `F64Bits` key class, so per-OCCURRENCE NaN dedup works at ANY depth.
/// No `f64` arithmetic: the id is packed into the payload field
/// (`0x7FF8_0000_0000_0000 | id`, ids up to 2^52 − 1 per call).
fn assign_nan_ids(qv: &QueryValue, next: &mut u64) -> QueryValue {
    match qv {
        QueryValue::F64(f) if f.is_nan() => {
            let id = (*next & 0x000f_ffff_ffff_ffff) + 1;
            *next += 1;
            QueryValue::F64(f64::from_bits(0x7ff8_0000_0000_0000 | id))
        }
        QueryValue::List(l) => {
            QueryValue::List(l.iter().map(|v| assign_nan_ids(v, next)).collect())
        }
        QueryValue::Set(s) => QueryValue::Set(s.iter().map(|v| assign_nan_ids(v, next)).collect()),
        QueryValue::Map(m) => QueryValue::Map(
            m.iter()
                .map(|(k, v)| (k.clone(), assign_nan_ids(v, next)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Remove duplicate `QueryValue` rows using the exact canonical numeric
/// key (`HashableQueryValue`, RV-FUNCLIB-5FA455F7) for deduplication.
///
/// NaN is per-OCCURRENCE distinct (NaN != NaN under the ONE rule) at ANY
/// depth: a pre-pass assigns every NaN leaf (top-level or nested inside a
/// List/Set/Map) a globally-unique occurrence id encoded in its payload
/// bits, and dedup runs on the TRANSFORMED records while the ORIGINALS are
/// returned. The transform (and its clone) happens only when a record
/// actually contains a NaN leaf — the common no-NaN path stays clone-free.
pub fn apply_distinct_qv(records: Vec<QueryValue>) -> Vec<QueryValue> {
    // Two-pass borrow pattern: HashableQueryValue<'a> borrows &'a QueryValue,
    // so we can't hold both the map key (reference into records) and own records
    // at the same time. Pass 1 walks references and records a keep-mask; pass 2
    // moves the kept records out by index order. The keep-mask is a single
    // densely-packed Vec<bool> — cheaper than a separate usize-keyed set.
    let n = records.len();
    let mut next_id: u64 = 0;
    // Parallel to `records`: the NaN-id-transformed copy, only for records
    // that contain a NaN leaf (`None` keeps the clone-free fast path).
    let transformed: Vec<Option<QueryValue>> = records
        .iter()
        .map(|record| {
            if contains_nan_leaf(record) {
                Some(assign_nan_ids(record, &mut next_id))
            } else {
                None
            }
        })
        .collect();
    let mut seen: IndexSet<HashableQueryValue<'_>, shamir_collections::THasher> =
        IndexSet::with_capacity_and_hasher(n, shamir_collections::THasher::default());
    let mut keep = vec![false; n];
    // Pass 1: walk references into records, mark first-occurrence indices.
    for (i, key) in records
        .iter()
        .zip(transformed.iter())
        .map(|(record, t)| HashableQueryValue(t.as_ref().unwrap_or(record)))
        .enumerate()
    {
        // Per-occurrence NaN: the transformed key is occurrence-unique, so
        // every NaN-bearing record is always kept.
        keep[i] = seen.insert(key);
    }
    // Pass 2: move kept records out in index order, preserving insertion order.
    records
        .into_iter()
        .zip(keep)
        .filter_map(|(v, k)| if k { Some(v) } else { None })
        .collect()
}
