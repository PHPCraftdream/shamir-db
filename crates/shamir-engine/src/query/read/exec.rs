//! Read query execution pipeline.
//!
//! Pipeline without GROUP BY:
//!   WHERE (filter_stream) → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult
//!
//! Pipeline with GROUP BY:
//!   WHERE (filter_stream) → GROUP BY → AGG per group → HAVING → SELECT → DISTINCT → ORDER BY → PAGINATION → QueryResult

use serde_json as json;
use smallvec::SmallVec;

use crate::query::filter::eval::{compare_values, intern_field_path, resolve_field_ref};
use crate::query::filter::{compile_filter, FilterContext};
use crate::query::read::{
    AggFunc, AggregateField, GroupBy, NullsOrder, OrderBy, OrderDirection, Pagination,
    PaginationInfo, Select, SelectItem,
};
use shamir_types::codecs::interned::inner_to_json_value;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

/// Typed hashable key fragment used to bucket records under GROUP BY.
///
/// The old keying serialised each group-field through
/// `inner_to_json_value -> json::Value::to_string -> Vec::join("|")`,
/// allocating a fresh `String` per record. For scalar group fields
/// (Int / Bool / Str / etc.) that's pointless — they're already
/// hashable. `GroupKeyItem` carries them through to a `TMap`
/// (`IndexMap<_, _, FxHasher>`) directly. Composite (Map / List /
/// Set / Dec / Big) group fields stay rare; they fall back to a
/// `Box<str>` JSON canonical form.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeyItem {
    Missing,
    Null,
    Bool(bool),
    Int(i64),
    F64Bits(u64),
    Str(Box<str>),
    Bin(Box<[u8]>),
    Complex(Box<str>),
}

fn group_key_item(val: Option<&InnerValue>, interner: &Interner) -> GroupKeyItem {
    match val {
        None => GroupKeyItem::Missing,
        Some(InnerValue::Null) => GroupKeyItem::Null,
        Some(InnerValue::Bool(b)) => GroupKeyItem::Bool(*b),
        Some(InnerValue::Int(i)) => GroupKeyItem::Int(*i),
        Some(InnerValue::F64(f)) => GroupKeyItem::F64Bits(f.to_bits()),
        Some(InnerValue::Str(s)) => GroupKeyItem::Str(s.as_str().into()),
        Some(InnerValue::Bin(b)) => GroupKeyItem::Bin(b.as_slice().into()),
        Some(other) => {
            // Fall back to JSON canonical form for non-scalar leaves.
            // Rare in practice — GROUP BY on a Map/List/Set field is
            // unusual — but kept for parity with the previous code path.
            let jv = inner_to_json_value(other, interner).unwrap_or(json::Value::Null);
            GroupKeyItem::Complex(jv.to_string().into_boxed_str())
        }
    }
}

// ============================================================================
// Select projection
// ============================================================================

/// Pre-resolved select projection info (avoids re-interning paths per record).
///
/// Output keys (alias or last path segment) are pre-allocated as
/// `String` at compile time — `project()` clones them per record
/// instead of paying `to_string()` for each field on each row.
pub struct SelectProjection {
    /// true → just convert whole record to JSON
    is_all: bool,
    /// (interned_path, pre-built output key)
    fields: Vec<(Option<Vec<u64>>, String)>,
}

impl SelectProjection {
    /// Build a reusable projection from a Select + Interner.
    pub fn new(select: &Select, interner: &Interner) -> Self {
        let is_all =
            select.items.is_empty() || select.items.iter().any(|i| matches!(i, SelectItem::All));

        let fields = if is_all {
            Vec::new()
        } else {
            select
                .items
                .iter()
                .filter_map(|item| match item {
                    SelectItem::Field { path, alias } => {
                        let interned = intern_field_path(path, interner);
                        let key = alias
                            .clone()
                            .unwrap_or_else(|| path.last().cloned().unwrap_or_default());
                        Some((interned, key))
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
            return inner_to_json_value(record, interner).unwrap_or(json::Value::Null);
        }
        if self.fields.is_empty() {
            return json::Value::Object(json::Map::new());
        }
        let mut obj = json::Map::new();
        for (interned_path, key) in &self.fields {
            let val = interned_path
                .as_ref()
                .and_then(|p| resolve_field_ref(record, p))
                .map(|v| inner_to_json_value(v, interner).unwrap_or(json::Value::Null))
                .unwrap_or(json::Value::Null);
            obj.insert(key.clone(), val);
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
        matches!(
            item,
            SelectItem::Aggregate { .. } | SelectItem::CountAll { .. }
        )
    })
}

/// Pre-intern all output key names from the Select items into the interner.
///
/// This ensures that `compile_filter` (which uses `intern_field_path` / `get_ind`)
/// can resolve field paths that refer to aggregate output keys like "total_age".
fn pre_intern_select_keys(select: &Select, interner: &Interner) {
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
            _ => continue,
        };
        // touch_ind ensures the key is interned (creates if missing)
        let _ = interner.touch_ind(key);
    }
}

/// Per-aggregate accumulator state. One instance per `Aggregate` select item
/// in a single group. The group is walked exactly once; every record feeds
/// every accumulator via `step(record)` using borrowed `&InnerValue` lookups
/// (no per-aggregate `Vec<Option<InnerValue>>` allocation, no record clone).
///
/// `Count{All}` is *not* represented here — it never touches records, so the
/// caller short-circuits to a plain `group_len` counter.
struct AggAccum<'a> {
    /// Pre-interned field path (`None` for `AggregateField::All`).
    field_path: Option<Vec<u64>>,
    /// `AggregateField::All` → step() uses the whole record as the value.
    all_field: bool,
    state: AggState<'a>,
}

enum AggState<'a> {
    /// Count(field) → non-null count.  Count(All) is folded into the caller.
    Count { count: u64 },
    /// Sum: integer fast path, lifted to f64 when any float is seen.
    Sum {
        sum_i: i64,
        sum_f: f64,
        has_float: bool,
    },
    /// Avg: f64 accumulator + non-null numeric count.
    Avg { sum: f64, count: u64 },
    /// Min: keeps a borrow into the source record.
    Min { current: Option<&'a InnerValue> },
    /// Max: keeps a borrow into the source record.
    Max { current: Option<&'a InnerValue> },
}

impl<'a> AggAccum<'a> {
    fn new(func: AggFunc, field: &AggregateField, interner: &Interner) -> Self {
        let (field_path, all_field) = match field {
            AggregateField::Field(p) => (intern_field_path(p, interner), false),
            AggregateField::All => (None, true),
        };
        let state = match func {
            AggFunc::Count => AggState::Count { count: 0 },
            AggFunc::Sum => AggState::Sum {
                sum_i: 0,
                sum_f: 0.0,
                has_float: false,
            },
            AggFunc::Avg => AggState::Avg { sum: 0.0, count: 0 },
            AggFunc::Min => AggState::Min { current: None },
            AggFunc::Max => AggState::Max { current: None },
        };
        Self {
            field_path,
            all_field,
            state,
        }
    }

    #[inline]
    fn resolve<'r>(&self, record: &'r InnerValue) -> Option<&'r InnerValue>
    where
        'r: 'a,
    {
        if self.all_field {
            // Count(*) was special-cased away; the only remaining caller for
            // AggregateField::All here is Count(All) via SelectItem::Aggregate
            // (rare path; CountAll is the documented spelling). The record
            // itself is the "value" — never None, always counted.
            Some(record)
        } else {
            self.field_path
                .as_deref()
                .and_then(|p| resolve_field_ref(record, p))
        }
    }

    #[inline]
    fn step(&mut self, record: &'a InnerValue) {
        let val = self.resolve(record);
        match &mut self.state {
            AggState::Count { count } => {
                if val.is_some() {
                    *count += 1;
                }
            }
            AggState::Sum {
                sum_i,
                sum_f,
                has_float,
            } => {
                if let Some(v) = val {
                    match v {
                        InnerValue::Int(i) => *sum_i += *i,
                        InnerValue::F64(f) => {
                            *has_float = true;
                            *sum_f += *f;
                        }
                        _ => {}
                    }
                }
            }
            AggState::Avg { sum, count } => {
                if let Some(v) = val {
                    match v {
                        InnerValue::Int(i) => {
                            *sum += *i as f64;
                            *count += 1;
                        }
                        InnerValue::F64(f) => {
                            *sum += *f;
                            *count += 1;
                        }
                        _ => {}
                    }
                }
            }
            AggState::Min { current } => {
                if let Some(v) = val {
                    match current {
                        None => *current = Some(v),
                        Some(cur) => {
                            if let Some(std::cmp::Ordering::Less) = compare_values(v, cur) {
                                *current = Some(v);
                            }
                        }
                    }
                }
            }
            AggState::Max { current } => {
                if let Some(v) = val {
                    match current {
                        None => *current = Some(v),
                        Some(cur) => {
                            if let Some(std::cmp::Ordering::Greater) = compare_values(v, cur) {
                                *current = Some(v);
                            }
                        }
                    }
                }
            }
        }
    }

    fn finish(self, interner: &Interner) -> json::Value {
        match self.state {
            AggState::Count { count } => json::Value::Number(count.into()),
            AggState::Sum {
                sum_i,
                sum_f,
                has_float,
            } => {
                if has_float {
                    let total = sum_f + sum_i as f64;
                    serde_json::Number::from_f64(total)
                        .map(json::Value::Number)
                        .unwrap_or(json::Value::Null)
                } else {
                    json::Value::Number(sum_i.into())
                }
            }
            AggState::Avg { sum, count } => {
                if count == 0 {
                    json::Value::Null
                } else {
                    let avg = sum / count as f64;
                    serde_json::Number::from_f64(avg)
                        .map(json::Value::Number)
                        .unwrap_or(json::Value::Null)
                }
            }
            AggState::Min { current } | AggState::Max { current } => current
                .map(|v| inner_to_json_value(v, interner).unwrap_or(json::Value::Null))
                .unwrap_or(json::Value::Null),
        }
    }
}

/// Build a JSON object from select items for a group of records.
///
/// Single-pass aggregation: every `SelectItem::Aggregate` becomes one
/// `AggAccum` slot; the group is walked exactly once and each record feeds
/// every accumulator via borrowed `&InnerValue` lookups. The previous code
/// path called `compute_aggregate` per item — which allocated a
/// `Vec<Option<InnerValue>>` of the same length as the group and *cloned*
/// the whole record for `Count(All)` on every row. With A aggregates that
/// was O(G·R·A) clones; we now do O(G·R) borrowed lookups.
///
/// `SelectItem::CountAll` is short-circuited to `group_records.len()` — it
/// never touches the records at all (no `AggAccum` slot, no resolve).
fn build_aggregate_object(
    group_records: &[&InnerValue],
    select: &Select,
    group_key_values: Option<&[(String, json::Value)]>,
    interner: &Interner,
) -> json::Value {
    let mut obj = json::Map::new();

    // Add group key values if provided
    if let Some(keys) = group_key_values {
        for (key, val) in keys {
            obj.insert(key.clone(), val.clone());
        }
    }

    // First pass over select items: allocate accumulators and remember the
    // output key for each one. Default-name strings need to outlive the
    // step loop, so we materialise them here.
    let mut agg_slots: Vec<(String, AggAccum<'_>)> = Vec::new();
    // Field-projection fallbacks (in group context, normally already
    // populated from `group_key_values`). Recorded for a second pass so
    // we don't run them during the hot aggregation loop.
    let mut field_slots: Vec<(String, &Vec<String>)> = Vec::new();

    for item in &select.items {
        match item {
            SelectItem::CountAll { alias } => {
                let key = alias.as_deref().unwrap_or("count");
                obj.insert(
                    key.to_string(),
                    json::Value::Number(group_records.len().into()),
                );
            }
            SelectItem::Aggregate {
                func, field, alias, ..
            } => {
                let key = match alias {
                    Some(a) => a.clone(),
                    None => match field {
                        AggregateField::Field(f) => {
                            format!("{:?}_{}", func, f.join(".")).to_lowercase()
                        }
                        AggregateField::All => format!("{:?}", func).to_lowercase(),
                    },
                };
                agg_slots.push((key, AggAccum::new(*func, field, interner)));
            }
            SelectItem::Field { path, alias } => {
                let default_key = path.last().map(|s| s.as_str()).unwrap_or("");
                let key = alias.as_deref().unwrap_or(default_key);
                if !obj.contains_key(key) {
                    field_slots.push((key.to_string(), path));
                }
            }
            SelectItem::All | SelectItem::Expression { .. } => {}
        }
    }

    // Single walk over the group: feeds every aggregate accumulator at once.
    if !agg_slots.is_empty() {
        for record in group_records {
            for (_, acc) in agg_slots.iter_mut() {
                acc.step(record);
            }
        }
    }

    for (key, acc) in agg_slots {
        obj.insert(key, acc.finish(interner));
    }

    // Resolve field-projection fallbacks from the first record (rare path).
    for (key, path) in field_slots {
        let val = group_records
            .first()
            .and_then(|first| {
                intern_field_path(path, interner)
                    .as_deref()
                    .and_then(|p| resolve_field_ref(first, p))
                    .map(|v| inner_to_json_value(v, interner).unwrap_or(json::Value::Null))
            })
            .unwrap_or(json::Value::Null);
        obj.insert(key, val);
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
    let group_paths: Vec<(String, Option<Vec<u64>>)> = group_by
        .fields
        .iter()
        .map(|f| {
            let display_name = f.last().cloned().unwrap_or_default();
            (display_name, intern_field_path(f, interner))
        })
        .collect();

    // Build groups: typed `Vec<GroupKeyItem>` key drives an `IndexMap`
    // hashed via FxHash. Each group's JSON-shaped key values stay
    // alongside the record list so the output projection can read them
    // without re-hitting the records. The JSON key values are computed
    // only on first insertion (Vacant branch) — repeated records into
    // an existing group skip the rebuild.
    use indexmap::map::Entry;
    #[allow(clippy::type_complexity)] // grouped aggregate accumulator; clarity over brevity
    let mut groups: TMap<Vec<GroupKeyItem>, (Vec<&InnerValue>, Vec<(String, json::Value)>)> =
        new_map_wc(0);

    for (_, record) in records {
        let mut group_key = Vec::with_capacity(group_paths.len());
        for (_, interned_path) in &group_paths {
            let val_ref = interned_path
                .as_ref()
                .and_then(|p| resolve_field_ref(record, p));
            group_key.push(group_key_item(val_ref, interner));
        }

        match groups.entry(group_key) {
            Entry::Occupied(mut e) => {
                e.get_mut().0.push(record);
            }
            Entry::Vacant(v) => {
                let mut key_json_values = Vec::with_capacity(group_paths.len());
                for (field_name, interned_path) in &group_paths {
                    let val_ref = interned_path
                        .as_ref()
                        .and_then(|p| resolve_field_ref(record, p));
                    let json_val = val_ref
                        .map(|vv| inner_to_json_value(vv, interner).unwrap_or(json::Value::Null))
                        .unwrap_or(json::Value::Null);
                    key_json_values.push((field_name.clone(), json_val));
                }
                v.insert((vec![record], key_json_values));
            }
        }
    }

    // The previous `BTreeMap<String, _>` ordering produced alphabetical
    // group output; tests depend on that. IndexMap::sort_keys does the
    // same in-place — no second `paired` Vec, no extra collect/move.
    groups.sort_keys();

    let mut result: Vec<json::Value> = Vec::with_capacity(groups.len());
    for (_k, (recs, key_vals)) in &groups {
        result.push(build_aggregate_object(
            recs,
            select,
            Some(key_vals),
            interner,
        ));
    }

    // Apply HAVING filter
    if let Some(having_filter) = &group_by.having {
        // Pre-intern all output field names so compile_filter can resolve them.
        // json_to_inner interns keys during conversion, but compile_filter
        // resolves paths at compile time via intern_field_path (get_ind).
        pre_intern_select_keys(select, interner);

        let having_cb = compile_filter(having_filter, interner);
        result.retain(|json_obj| {
            // Walk `json::Value` straight into InnerValue — the old path
            // went through `serde_json::to_vec` + `json_to_inner` (parse
            // bytes back), which is a needless round-trip.
            shamir_types::codecs::interned::json_value_to_inner(json_obj, interner)
                .map(|inner| having_cb.matches(&inner, ctx))
                .unwrap_or(false)
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
///
/// Pre-resolves field values once per record (O(n) linear scan), then
/// sorts an index array by those pre-resolved references.  This avoids
/// repeated `Value::get` lookups inside the comparator — the dominant
/// cost identified in bench #106 (~85% of ORDER BY time).
pub fn apply_order_by(records: &mut Vec<json::Value>, order_by: &OrderBy) {
    if order_by.items.is_empty() || records.len() <= 1 {
        return;
    }

    // Phase 1: pre-resolve field values — one linear pass.
    // Stores raw pointers into the source records (zero-copy).
    let keys: Vec<PreResolvedKeys> = records
        .iter()
        .map(|r| resolve_order_keys(r, &order_by.items))
        .collect();

    // Phase 2: sort index array by pre-resolved keys.
    let mut idx: Vec<usize> = (0..records.len()).collect();
    idx.sort_by(|&a, &b| compare_preresolved(&keys[a], &keys[b], &order_by.items));

    // Phase 3: apply permutation in-place.
    let sorted: Vec<json::Value> = idx
        .into_iter()
        .map(|i| std::mem::take(&mut records[i]))
        .collect();
    *records = sorted;
}

/// Pre-resolved field values for all ORDER BY fields of one record.
/// SmallVec<[…; 4]> avoids heap allocation for the common ≤4 field case.
type PreResolvedKeys = SmallVec<[SortKey; 4]>;

/// Typed pre-resolved ORDER BY field value. The comparator dispatches on
/// the enum variant once and then compares native types (i64::cmp,
/// str::cmp, etc) — bypassing the per-comparison `serde_json::Value`
/// match that dominated the original `apply_order_by`.
#[derive(Clone)]
enum SortKey {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(Box<str>),
    Other, // unsupported (Array / Object) — falls back to Equal
}

impl SortKey {
    fn from_json(v: Option<&json::Value>) -> Self {
        match v {
            None | Some(json::Value::Null) => SortKey::Null,
            Some(json::Value::Bool(b)) => SortKey::Bool(*b),
            Some(json::Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    SortKey::I64(i)
                } else if let Some(f) = n.as_f64() {
                    SortKey::F64(f)
                } else {
                    SortKey::Null
                }
            }
            Some(json::Value::String(s)) => SortKey::Str(s.as_str().into()),
            _ => SortKey::Other,
        }
    }

    #[inline]
    fn is_null(&self) -> bool {
        matches!(self, SortKey::Null)
    }
}

/// Pre-resolve all ORDER BY field values from a single JSON record.
fn resolve_order_keys(
    record: &json::Value,
    items: &[crate::query::read::OrderByItem],
) -> PreResolvedKeys {
    items
        .iter()
        .map(|item| SortKey::from_json(get_json_field(record, &item.field)))
        .collect()
}

/// Compare two pre-resolved key vectors.
fn compare_preresolved(
    a: &PreResolvedKeys,
    b: &PreResolvedKeys,
    items: &[crate::query::read::OrderByItem],
) -> std::cmp::Ordering {
    for (i, item) in items.iter().enumerate() {
        let ord = compare_sort_keys(&a[i], &b[i], &item.direction, &item.nulls);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Compare two pre-resolved sort keys with direction + nulls handling.
/// Mirrors `compare_json_values` semantics but dispatches on the typed
/// enum once instead of matching `serde_json::Value` on every comparison.
#[inline]
fn compare_sort_keys(
    a: &SortKey,
    b: &SortKey,
    direction: &OrderDirection,
    nulls: &Option<NullsOrder>,
) -> std::cmp::Ordering {
    let is_null_a = a.is_null();
    let is_null_b = b.is_null();
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

    let base = match (a, b) {
        (SortKey::I64(x), SortKey::I64(y)) => x.cmp(y),
        (SortKey::F64(x), SortKey::F64(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::I64(x), SortKey::F64(y)) => (*x as f64)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::F64(x), SortKey::I64(y)) => x
            .partial_cmp(&(*y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (SortKey::Str(x), SortKey::Str(y)) => x.as_ref().cmp(y.as_ref()),
        (SortKey::Bool(x), SortKey::Bool(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    };

    match direction {
        OrderDirection::Asc => base,
        OrderDirection::Desc => base.reverse(),
    }
}

/// Get a field from a JSON value by path segments.
fn get_json_field<'a>(value: &'a json::Value, path: &[String]) -> Option<&'a json::Value> {
    let mut current = value;
    for part in path {
        current = current.get(part.as_str())?;
    }
    Some(current)
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

    let sliced: Vec<json::Value> = {
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

/// Wrapper that gives `json::Value` a `Hash + Eq` implementation backed by
/// a structural walk of the tree. `json::Value::eq` is structural already;
/// the missing piece was `Hash`, which the standard library can't provide
/// because `serde_json::Number` carries non-totally-ordered floats. We
/// hash float bits — the same canonical form the old `to_string()` path
/// produced, just without allocating a `String` per record.
struct HashableJson(json::Value);

impl PartialEq for HashableJson {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for HashableJson {}

impl std::hash::Hash for HashableJson {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        hash_json(&self.0, h);
    }
}

fn hash_json<H: std::hash::Hasher>(v: &json::Value, h: &mut H) {
    use std::hash::Hash;
    match v {
        json::Value::Null => h.write_u8(0),
        json::Value::Bool(b) => {
            h.write_u8(1);
            h.write_u8(*b as u8);
        }
        json::Value::Number(n) => {
            h.write_u8(2);
            if let Some(i) = n.as_i64() {
                h.write_u8(0);
                h.write_i64(i);
            } else if let Some(u) = n.as_u64() {
                h.write_u8(1);
                h.write_u64(u);
            } else if let Some(f) = n.as_f64() {
                h.write_u8(2);
                h.write_u64(f.to_bits());
            } else {
                h.write_u8(3);
                // Falls back through Display; rare path.
                n.to_string().hash(h);
            }
        }
        json::Value::String(s) => {
            h.write_u8(3);
            h.write(s.as_bytes());
            h.write_u8(0xff);
        }
        json::Value::Array(arr) => {
            h.write_u8(4);
            h.write_u64(arr.len() as u64);
            for x in arr {
                hash_json(x, h);
            }
        }
        json::Value::Object(map) => {
            h.write_u8(5);
            h.write_u64(map.len() as u64);
            for (k, v) in map {
                h.write(k.as_bytes());
                h.write_u8(0);
                hash_json(v, h);
            }
        }
    }
}

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
