//! Read query execution pipeline.
//!
//! Pipeline without GROUP BY:
//!   WHERE (filter_stream) → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult
//!
//! Pipeline with GROUP BY:
//!   WHERE (filter_stream) → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult

pub use crate::query::read::aggregate::{apply_aggregate_all, apply_group_by};
use crate::query::read::hashable_query_value::HashableQueryValue;
pub use crate::query::read::order::{apply_order_by_qv, apply_order_by_topk};
pub use crate::query::read::select_projection::SelectProjection;
pub use crate::query::read::{Pagination, PaginationInfo, Select, SelectItem};
use indexmap::IndexSet;
use shamir_funclib::scalar_resolver::ScalarResolver;
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
pub fn apply_select_value(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
    scalars: ScalarResolver,
) -> Vec<QueryValue> {
    let proj = SelectProjection::new(select, interner, scalars);
    records
        .iter()
        .map(|(_, record)| proj.project_value(record, interner))
        .collect()
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

/// Remove duplicate `QueryValue` rows using a canonical key for
/// deduplication. The canonical key reproduces the lossy
/// coercion applied historically (Dec/Big→String, Bin→Array, Set→Array)
/// so that e.g. `Dec("1.0")` and `Str("1.0")` deduplicate identically.
pub fn apply_distinct_qv(records: Vec<QueryValue>) -> Vec<QueryValue> {
    // Two-pass borrow pattern: HashableQueryValue<'a> borrows &'a QueryValue,
    // so we can't hold both the map key (reference into records) and own records
    // at the same time. Pass 1 walks references and records a keep-mask; pass 2
    // moves the kept records out by index order. The keep-mask is a single
    // densely-packed Vec<bool> — cheaper than a separate usize-keyed set.
    let n = records.len();
    let mut seen: IndexSet<HashableQueryValue<'_>, shamir_collections::THasher> =
        IndexSet::with_capacity_and_hasher(n, shamir_collections::THasher::default());
    let mut keep = vec![false; n];
    // Pass 1: walk references into records, mark first-occurrence indices.
    for (i, record) in records.iter().enumerate() {
        if seen.insert(HashableQueryValue(record)) {
            keep[i] = true;
        }
    }
    // Pass 2: move kept records out in index order, preserving insertion order.
    records
        .into_iter()
        .zip(keep)
        .filter_map(|(v, k)| if k { Some(v) } else { None })
        .collect()
}
