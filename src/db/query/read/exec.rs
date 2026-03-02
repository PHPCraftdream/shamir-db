//! Read query execution pipeline.
//!
//! Pipeline without GROUP BY:
//!   WHERE (filter_stream) → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult
//!
//! Pipeline with GROUP BY:
//!   WHERE (filter_stream) → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult

use std::collections::BTreeMap;

use serde_json as json;

use crate::codecs::interned::inner_to_json_value;
use crate::core::interner::Interner;
use crate::db::query::filter::eval::{compare_values, intern_field_path, resolve_field};
use crate::db::query::filter::{compile_filter, FilterContext};
use crate::db::query::read::{
    AggFunc, AggregateField, GroupBy, NullsOrder, OrderBy, OrderDirection, Pagination,
    PaginationInfo, Select, SelectItem,
};
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;

// ============================================================================
// Select projection
// ============================================================================

/// Pre-resolved select projection info (avoids re-interning paths per record).
pub struct SelectProjection {
    /// true → just convert whole record to JSON
    is_all: bool,
    /// (interned_path, raw_path, alias)
    fields: Vec<(Option<Vec<u64>>, String, Option<String>)>,
}

impl SelectProjection {
    /// Build a reusable projection from a Select + Interner.
    pub fn new(select: &Select, interner: &Interner) -> Self {
        let is_all = select.items.is_empty()
            || select.items.iter().any(|i| matches!(i, SelectItem::All));

        let fields = if is_all {
            Vec::new()
        } else {
            select
                .items
                .iter()
                .filter_map(|item| match item {
                    SelectItem::Field { path, alias } => {
                        let interned = intern_field_path(path, interner);
                        Some((interned, path.clone(), alias.clone()))
                    }
                    _ => None,
                })
                .collect()
        };

        Self { is_all, fields }
    }

    /// Project a single InnerValue record to JSON.
    pub fn project(&self, record: &InnerValue, interner: &Interner) -> json::Value {
        if self.is_all {
            return inner_to_json_value(record, interner);
        }
        if self.fields.is_empty() {
            return json::Value::Object(json::Map::new());
        }
        let mut obj = json::Map::new();
        for (interned_path, raw_path, alias) in &self.fields {
            let val = interned_path
                .as_ref()
                .and_then(|p| resolve_field(record, p))
                .map(|v| inner_to_json_value(&v, interner))
                .unwrap_or(json::Value::Null);
            let key = alias.as_deref().unwrap_or(raw_path);
            obj.insert(key.to_string(), val);
        }
        json::Value::Object(obj)
    }
}

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

// ============================================================================
// Aggregation helpers
// ============================================================================

/// Check whether the select list contains any aggregates.
pub fn has_aggregates(select: &Select) -> bool {
    select.items.iter().any(|item| {
        matches!(item, SelectItem::Aggregate { .. } | SelectItem::CountAll { .. })
    })
}

/// Pre-intern all output key names from the Select items into the interner.
///
/// This ensures that `compile_filter` (which uses `intern_field_path` / `get_ind`)
/// can resolve field paths that refer to aggregate output keys like "total_age".
fn pre_intern_select_keys(select: &Select, interner: &Interner) {
    for item in &select.items {
        let key = match item {
            SelectItem::Field { path, alias } => alias.as_deref().unwrap_or(path),
            SelectItem::CountAll { alias } => alias.as_deref().unwrap_or("count"),
            SelectItem::Aggregate {
                alias, ..
            } => {
                if let Some(a) = alias {
                    a.as_str()
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        // touch_ind ensures the key is interned (creates if missing)
        let _ = interner.touch_ind(key);
    }
}

/// Compute a single aggregate over a slice of InnerValues.
fn compute_aggregate(
    values: &[&InnerValue],
    func: &AggFunc,
    field: &AggregateField,
    interner: &Interner,
) -> json::Value {
    let field_path = match field {
        AggregateField::Field(path) => intern_field_path(path, interner),
        AggregateField::All => None,
    };

    // Extract field values from each record
    let field_values: Vec<Option<InnerValue>> = values
        .iter()
        .map(|record| {
            if let Some(ref path) = field_path {
                resolve_field(record, path)
            } else {
                // All → the record itself (meaningful only for Count)
                Some((*record).clone())
            }
        })
        .collect();

    match func {
        AggFunc::Count => {
            let count = field_values.iter().filter(|v| v.is_some()).count();
            json::Value::Number(count.into())
        }
        AggFunc::Sum => {
            let mut sum_i: i64 = 0;
            let mut sum_f: f64 = 0.0;
            let mut has_float = false;
            for val in field_values.iter().flatten() {
                match val {
                    InnerValue::Int(i) => sum_i += i,
                    InnerValue::F64(f) => {
                        has_float = true;
                        sum_f += f;
                    }
                    _ => {}
                }
            }
            if has_float {
                let total = sum_f + sum_i as f64;
                serde_json::Number::from_f64(total)
                    .map(json::Value::Number)
                    .unwrap_or(json::Value::Null)
            } else {
                json::Value::Number(sum_i.into())
            }
        }
        AggFunc::Avg => {
            let mut sum: f64 = 0.0;
            let mut count: u64 = 0;
            for val in field_values.iter().flatten() {
                match val {
                    InnerValue::Int(i) => {
                        sum += *i as f64;
                        count += 1;
                    }
                    InnerValue::F64(f) => {
                        sum += f;
                        count += 1;
                    }
                    _ => {}
                }
            }
            if count == 0 {
                json::Value::Null
            } else {
                let avg = sum / count as f64;
                serde_json::Number::from_f64(avg)
                    .map(json::Value::Number)
                    .unwrap_or(json::Value::Null)
            }
        }
        AggFunc::Min => {
            let mut min: Option<&InnerValue> = None;
            for val in field_values.iter().flatten() {
                match min {
                    None => min = Some(val),
                    Some(current) => {
                        if let Some(std::cmp::Ordering::Less) = compare_values(val, current) {
                            min = Some(val);
                        }
                    }
                }
            }
            min.map(|v| inner_to_json_value(v, interner))
                .unwrap_or(json::Value::Null)
        }
        AggFunc::Max => {
            let mut max: Option<&InnerValue> = None;
            for val in field_values.iter().flatten() {
                match max {
                    None => max = Some(val),
                    Some(current) => {
                        if let Some(std::cmp::Ordering::Greater) = compare_values(val, current) {
                            max = Some(val);
                        }
                    }
                }
            }
            max.map(|v| inner_to_json_value(v, interner))
                .unwrap_or(json::Value::Null)
        }
    }
}

/// Build a JSON object from select items for a group of records.
fn build_aggregate_object(
    group_records: &[&InnerValue],
    select: &Select,
    group_key_values: Option<&[(&str, json::Value)]>,
    interner: &Interner,
) -> json::Value {
    let mut obj = json::Map::new();

    // Add group key values if provided
    if let Some(keys) = group_key_values {
        for (key, val) in keys {
            obj.insert(key.to_string(), val.clone());
        }
    }

    for item in &select.items {
        match item {
            SelectItem::CountAll { alias } => {
                let key = alias.as_deref().unwrap_or("count");
                obj.insert(key.to_string(), json::Value::Number(group_records.len().into()));
            }
            SelectItem::Aggregate {
                func,
                field,
                alias,
                ..
            } => {
                let default_name = match (func, field) {
                    (_, AggregateField::Field(f)) => format!("{:?}_{}", func, f).to_lowercase(),
                    (_, AggregateField::All) => format!("{:?}", func).to_lowercase(),
                };
                let key = alias.as_deref().unwrap_or(&default_name);
                let val = compute_aggregate(group_records, func, field, interner);
                obj.insert(key.to_string(), val);
            }
            SelectItem::Field { path, alias } => {
                // In group context, field must be a group-by field.
                // Already added from group_key_values, but handle case
                // where it wasn't in group_key_values
                let key = alias.as_deref().unwrap_or(path);
                if !obj.contains_key(key) {
                    // Take value from first record
                    if let Some(first) = group_records.first() {
                        if let Some(interned) = intern_field_path(path, interner) {
                            let val = resolve_field(first, &interned)
                                .map(|v| inner_to_json_value(&v, interner))
                                .unwrap_or(json::Value::Null);
                            obj.insert(key.to_string(), val);
                        } else {
                            obj.insert(key.to_string(), json::Value::Null);
                        }
                    }
                }
            }
            SelectItem::All | SelectItem::Expression { .. } => {}
        }
    }

    json::Value::Object(obj)
}

// ============================================================================
// Group By
// ============================================================================

/// Apply GROUP BY + aggregation + HAVING.
pub fn apply_group_by(
    records: &[(RecordId, InnerValue)],
    group_by: &GroupBy,
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> Vec<json::Value> {
    if records.is_empty() {
        return Vec::new();
    }

    // Pre-intern group-by field paths
    let group_paths: Vec<(&str, Option<Vec<u64>>)> = group_by
        .fields
        .iter()
        .map(|f| (f.as_str(), intern_field_path(f, interner)))
        .collect();

    // Build groups: key = serialized group values, value = vec of record refs
    let mut groups: BTreeMap<String, Vec<&InnerValue>> = BTreeMap::new();
    // Also store the JSON key values per group for output
    let mut group_keys_map: BTreeMap<String, Vec<(&str, json::Value)>> = BTreeMap::new();

    for (_, record) in records {
        let mut key_parts = Vec::with_capacity(group_paths.len());
        let mut key_json_values = Vec::with_capacity(group_paths.len());

        for (field_name, interned_path) in &group_paths {
            let val = interned_path
                .as_ref()
                .and_then(|p| resolve_field(record, p));
            let json_val = val
                .as_ref()
                .map(|v| inner_to_json_value(v, interner))
                .unwrap_or(json::Value::Null);
            // Use canonical JSON for grouping key
            key_parts.push(json_val.to_string());
            key_json_values.push((*field_name, json_val));
        }

        let group_key = key_parts.join("|");
        groups.entry(group_key.clone()).or_default().push(record);
        group_keys_map
            .entry(group_key)
            .or_insert(key_json_values);
    }

    // Build result for each group
    let mut result: Vec<json::Value> = Vec::with_capacity(groups.len());

    for (key, group_records) in &groups {
        let key_values = group_keys_map.get(key).map(|v| v.as_slice());
        let obj = build_aggregate_object(group_records, select, key_values, interner);
        result.push(obj);
    }

    // Apply HAVING filter
    if let Some(having_filter) = &group_by.having {
        // Pre-intern all output field names so compile_filter can resolve them.
        // json_to_inner interns keys during conversion, but compile_filter
        // resolves paths at compile time via intern_field_path (get_ind).
        pre_intern_select_keys(select, interner);

        let having_cb = compile_filter(having_filter, interner);
        result.retain(|json_obj| {
            // Convert JSON back to InnerValue for filter evaluation
            if let Ok(bytes) = serde_json::to_vec(json_obj) {
                if let Ok(inner) = crate::codecs::interned::json_to_inner(interner, &bytes) {
                    return having_cb.matches(&inner, ctx);
                }
            }
            false
        });
    }

    result
}

// ============================================================================
// Aggregate All (no GROUP BY but aggregates in SELECT)
// ============================================================================

/// When SELECT contains aggregates but no GROUP BY — aggregate over the entire set.
pub fn apply_aggregate_all(
    records: &[(RecordId, InnerValue)],
    select: &Select,
    interner: &Interner,
) -> Vec<json::Value> {
    let refs: Vec<&InnerValue> = records.iter().map(|(_, v)| v).collect();
    let obj = build_aggregate_object(&refs, select, None, interner);
    vec![obj]
}

// ============================================================================
// Order By
// ============================================================================

/// Sort JSON objects by ORDER BY items.
pub fn apply_order_by(records: &mut [json::Value], order_by: &OrderBy) {
    records.sort_by(|a, b| {
        for item in &order_by.items {
            let va = get_json_field(a, &item.field);
            let vb = get_json_field(b, &item.field);

            let ord = compare_json_values(va, vb, &item.direction, &item.nulls);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// Get a field from a JSON value by dot-separated path.
fn get_json_field<'a>(value: &'a json::Value, path: &str) -> Option<&'a json::Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Compare two JSON values for ordering.
fn compare_json_values(
    a: Option<&json::Value>,
    b: Option<&json::Value>,
    direction: &OrderDirection,
    nulls: &Option<NullsOrder>,
) -> std::cmp::Ordering {
    let is_null_a = a.is_none() || matches!(a, Some(json::Value::Null));
    let is_null_b = b.is_none() || matches!(b, Some(json::Value::Null));

    // Handle nulls
    if is_null_a && is_null_b {
        return std::cmp::Ordering::Equal;
    }
    if is_null_a || is_null_b {
        let nulls_order = nulls.unwrap_or(match direction {
            OrderDirection::Asc => NullsOrder::Last,
            OrderDirection::Desc => NullsOrder::First,
        });
        let null_first = matches!(nulls_order, NullsOrder::First);
        return if is_null_a == null_first {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        };
    }

    let a = a.unwrap();
    let b = b.unwrap();

    let base = match (a, b) {
        (json::Value::Number(na), json::Value::Number(nb)) => {
            let fa = na.as_f64().unwrap_or(0.0);
            let fb = nb.as_f64().unwrap_or(0.0);
            fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
        }
        (json::Value::String(sa), json::Value::String(sb)) => sa.cmp(sb),
        (json::Value::Bool(ba), json::Value::Bool(bb)) => ba.cmp(bb),
        _ => std::cmp::Ordering::Equal,
    };

    match direction {
        OrderDirection::Asc => base,
        OrderDirection::Desc => base.reverse(),
    }
}

// ============================================================================
// Pagination
// ============================================================================

/// Apply pagination to results, returning (page_records, pagination_info).
pub fn apply_pagination(
    records: Vec<json::Value>,
    pagination: &Pagination,
    count_total: bool,
) -> (Vec<json::Value>, Option<PaginationInfo>) {
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

    let sliced: Vec<json::Value> = if let Some(limit) = take {
        records.into_iter().skip(skip).take(limit as usize).collect()
    } else {
        records.into_iter().skip(skip).collect()
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

/// Remove duplicate JSON values (based on canonical string representation).
pub fn apply_distinct(records: Vec<json::Value>) -> Vec<json::Value> {
    let mut seen = indexmap::IndexSet::new();
    let mut result = Vec::with_capacity(records.len());

    for record in records {
        let canonical = record.to_string();
        if seen.insert(canonical) {
            result.push(record);
        }
    }

    result
}
