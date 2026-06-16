//! Aggregation helpers: accumulators, GROUP BY, aggregate-all.

use indexmap::map::Entry;
use serde_json as json;

use crate::function::builtin_aggs;
use crate::query::filter::eval::{intern_field_path, resolve_field_ref, resolve_filter_value};
use crate::query::filter::{compile_filter, FilterContext, FilterValue, FnCall};
use crate::query::read::exec::pre_intern_select_keys;
use crate::query::read::{AggFunc, AggregateField, GroupBy, QueryResult, Select, SelectItem};
use shamir_funclib::agg::Aggregator;
use shamir_types::codecs::interned::inner_to_json_value;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval::compare_values;

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
pub(super) enum GroupKeyItem {
    Missing,
    Null,
    Bool(bool),
    Int(i64),
    F64Bits(u64),
    Str(Box<str>),
    Bin(Box<[u8]>),
    Complex(Box<str>),
}

pub(super) fn group_key_item(val: Option<&InnerValue>, interner: &Interner) -> GroupKeyItem {
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

/// Per-aggregate accumulator state. One instance per `Aggregate` select item
/// in a single group. The group is walked exactly once; every record feeds
/// every accumulator via `step(record)` using borrowed `&InnerValue` lookups
/// (no per-aggregate `Vec<Option<InnerValue>>` allocation, no record clone).
///
/// `Count{All}` is *not* represented here — it never touches records, so the
/// caller short-circuits to a plain `group_len` counter.
pub(super) struct AggAccum<'a> {
    /// Pre-interned field path (`None` for `AggregateField::All`).
    field_path: Option<Vec<u64>>,
    /// `AggregateField::All` → step() uses the whole record as the value.
    all_field: bool,
    state: AggState<'a>,
}

pub(super) enum AggState<'a> {
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
    pub(super) fn new(func: AggFunc, field: &AggregateField, interner: &Interner) -> Self {
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
    pub(super) fn step(&mut self, record: &'a InnerValue) {
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

    pub(super) fn finish(self, interner: &Interner) -> json::Value {
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
pub(super) fn build_aggregate_object(
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

    // funclib aggregate slots: (output key, interned field path, whole-record
    // flag, freshly-minted aggregator). `None` aggregator = unknown name →
    // the cell finalises to Null rather than panicking.
    #[allow(clippy::type_complexity)] // parallel to agg_slots; clarity over brevity
    let mut fn_slots: Vec<(String, Option<Vec<u64>>, bool, Option<Box<dyn Aggregator>>)> =
        Vec::new();

    // Scalar-function select items in a group context: evaluated once against
    // the group's representative (first) record — same fallback as plain
    // field projections in a group.
    let mut func_slots: Vec<(String, FilterValue)> = Vec::new();

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
            SelectItem::AggregateFn {
                name, field, alias, ..
            } => {
                let key = match alias {
                    Some(a) => a.clone(),
                    None => match field {
                        AggregateField::Field(f) => format!("{}_{}", name, f.join(".")),
                        AggregateField::All => name.clone(),
                    },
                };
                let (field_path, all_field) = match field {
                    AggregateField::Field(p) => (intern_field_path(p, interner), false),
                    AggregateField::All => (None, true),
                };
                fn_slots.push((key, field_path, all_field, builtin_aggs().make(name)));
            }
            SelectItem::Function { name, args, alias } => {
                let key = alias.clone().unwrap_or_else(|| name.clone());
                let fv = FilterValue::FnCall {
                    call: FnCall::complex(name.clone(), args.clone()),
                };
                func_slots.push((key, fv));
            }
            SelectItem::All | SelectItem::Expression { .. } => {}
        }
    }

    // Single walk over the group: feeds every aggregate accumulator at once
    // (both the built-in fast-path slots and the funclib aggregate slots).
    if !agg_slots.is_empty() || !fn_slots.is_empty() {
        for record in group_records {
            for (_, acc) in agg_slots.iter_mut() {
                acc.step(record);
            }
            for (_, path, all_field, agg) in fn_slots.iter_mut() {
                if let Some(agg) = agg {
                    let val = if *all_field {
                        Some(*record)
                    } else {
                        path.as_deref().and_then(|p| resolve_field_ref(record, p))
                    };
                    if let Some(v) = val {
                        // Aggregator errors (e.g. type_mismatch) are swallowed
                        // per-row; finalize() decides the cell's final value.
                        let _ = agg.accumulate(v);
                    }
                }
            }
        }
    }

    for (key, acc) in agg_slots {
        obj.insert(key, acc.finish(interner));
    }

    for (key, _, _, agg) in fn_slots {
        let jv = match agg {
            Some(agg) => match agg.finalize() {
                Ok(v) => inner_to_json_value(&v, interner).unwrap_or(json::Value::Null),
                Err(_) => json::Value::Null,
            },
            None => json::Value::Null,
        };
        obj.insert(key, jv);
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

    // Scalar functions in a group SELECT: evaluate against the group's first
    // record (mirrors the field-projection fallback above).
    if !func_slots.is_empty() {
        let empty_refs: TMap<String, QueryResult> = new_map_wc(0);
        let ctx = FilterContext::new(interner, &empty_refs);
        for (key, fv) in func_slots {
            let val = group_records
                .first()
                .and_then(|first| resolve_filter_value(&fv, *first, &ctx))
                .map(|v| inner_to_json_value(&v, interner).unwrap_or(json::Value::Null))
                .unwrap_or(json::Value::Null);
            obj.insert(key, val);
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
