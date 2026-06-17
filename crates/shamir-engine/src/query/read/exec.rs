//! Read query execution pipeline.
//!
//! Pipeline without GROUP BY:
//!   WHERE (filter_stream) → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult
//!
//! Pipeline with GROUP BY:
//!   WHERE (filter_stream) → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult

use serde_json as json;

pub use crate::query::read::aggregate::{apply_aggregate_all, apply_group_by};
use crate::query::read::hashable_json::HashableJson;
pub use crate::query::read::order::{apply_order_by, apply_order_by_qv};
pub use crate::query::read::select_projection::SelectProjection;
use crate::query::read::{Pagination, PaginationInfo, Select, SelectItem};
use shamir_types::core::interner::Interner;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

// ============================================================================
// Select projection (public API)
// ============================================================================

/// Apply SELECT projection to raw records, producing JSON values.
///
/// Aggregate items (Aggregate, CountAll) are skipped here — they are handled
/// by `apply_group_by` or `apply_aggregate_all`.
pub fn apply_select(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
) -> Vec<json::Value> {
    let proj = SelectProjection::new(select, interner);
    records
        .iter()
        .map(|(_, record)| proj.project(record, interner))
        .collect()
}

/// Streaming variant of `apply_select`: projects records and serialises
/// each directly to JSON bytes via `inner_to_json` — bypassing the
/// intermediate `json::Value` tree. Returns the same content as
/// `serde_json::to_vec(&apply_select(...))` but in one pass for SELECT *.
///
/// Fast path: when `select` is `SELECT *` (all fields, no
/// aggregates/functions), each record is serialised directly from its
/// `InnerValue` via `inner_to_json`, which uses `InternedRef` (a zero-copy
/// streaming Serialize) and never builds a `json::Value` tree.
///
/// General path (non-* selects): falls back to `apply_select` + `to_vec`.
pub fn apply_select_to_bytes(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
) -> Vec<u8> {
    use shamir_types::codecs::interned::json::inner_to_json;
    // Fast path: SELECT * — serialise InnerValue directly, no json::Value.
    let is_all =
        select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All));
    if is_all {
        let mut buf = Vec::with_capacity(records.len() * 200 + 2);
        buf.push(b'[');
        for (i, (_, record)) in records.iter().enumerate() {
            if i > 0 {
                buf.push(b',');
            }
            match inner_to_json(interner, record) {
                Ok(bytes) => buf.extend_from_slice(&bytes),
                Err(_) => buf.extend_from_slice(b"null"),
            }
        }
        buf.push(b']');
        return buf;
    }
    // General path: project to json::Value tree, then serialise.
    let projected = apply_select(records, select, interner);
    json::to_vec(&projected).unwrap_or_default()
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

/// Remove duplicate JSON values. Walks each value's structure for the
/// hash instead of `record.to_string()` — no per-record JSON
/// serialisation, no per-record `String` allocation.
pub fn apply_distinct(records: Vec<json::Value>) -> Vec<json::Value> {
    type Set = indexmap::IndexSet<HashableJson, std::hash::BuildHasherDefault<fxhash::FxHasher>>;
    let mut seen: Set = indexmap::IndexSet::with_capacity_and_hasher(
        records.len(),
        std::hash::BuildHasherDefault::default(),
    );
    for record in records {
        seen.insert(HashableJson(record));
    }
    seen.into_iter().map(|h| h.0).collect()
}

/// Remove duplicate `QueryValue` rows using a canonical json key for
/// deduplication, matching the semantics of the json-based
/// `apply_distinct`. The canonical key reproduces the lossy
/// `From<QueryValue> for serde_json::Value` coercion (Dec/Big→String,
/// Bin→Array, Set→Array) so that e.g. `Dec("1.0")` and `Str("1.0")`
/// deduplicate identically to the old json path.
pub fn apply_distinct_qv(records: Vec<QueryValue>) -> Vec<QueryValue> {
    type Map = indexmap::IndexMap<HashableJson, QueryValue, shamir_collections::THasher>;
    let mut seen: Map = indexmap::IndexMap::with_capacity_and_hasher(
        records.len(),
        shamir_collections::THasher::default(),
    );
    for record in records {
        let key = HashableJson(json::Value::from(record.clone()));
        seen.entry(key).or_insert(record);
    }
    seen.into_values().collect()
}
