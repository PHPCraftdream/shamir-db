//! Aggregation helpers: accumulators, GROUP BY, aggregate-all.
//!
//! S4 (#76) — the aggregation pipeline is now lens-fed. Each row reaches the
//! accumulators as raw storage `Bytes`; a per-row `RecordView` (the zero-copy
//! msgpack lens) supplies every aggregated/group field via `scalar_at` →
//! `ScalarRef`. Owned state survives only where §5b of the anti-formal doc
//! demands it:
//!
//! - **Min/Max** hold the running extreme as a small `OwnedScalar` (leaf bytes
//!   / scalar), not a `Value<K>` tree — the incoming `ScalarRef` is compared
//!   against it directly.
//! - **Sum/Avg/Count** keep their numeric accumulators; the input is read as
//!   `ScalarRef::Int`/`F64` straight off the lens — no `Value` materialised.
//! - **funclib aggregate slots** are fed `QueryValue` built at the funclib
//!   boundary only (scalar ⇒ cheap; container ⇒ one `materialize_at` per row).
//! - **HAVING** compares the predicate against the `QueryValue` result map via
//!   a `RecordRef` adapter that re-keys String → `InternerKey` once and leaves
//!   the leaves as `QueryValue` — the `query_value_to_inner` bridge is gone.

use std::cmp::Ordering;

use bytes::Bytes;
use indexmap::map::Entry;
use serde_json as json;

use crate::function::builtin_aggs;
use crate::query::filter::eval::{compare_values, resolve_filter_value};
use crate::query::filter::{compile_filter, FilterContext, FilterValue, FnCall};
use crate::query::read::exec::pre_intern_select_keys;
use crate::query::read::{AggFunc, AggregateField, GroupBy, QueryResult, Select, SelectItem};
use shamir_funclib::agg::Aggregator;
use shamir_types::codecs::interned::{inner_to_json_value, inner_value_to_query_value};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{HavingView, RecordRef, RecordView, ScalarRef};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

// ============================================================================
// OwnedExtreme — the running-state currency for Min/Max (§5b boundary #2).
// ============================================================================

/// The running extreme held across rows by Min/Max. Two reprs:
///
/// - `Scalar` — the overwhelming common case (Min/Max over Int/F64/Str/Bool).
///   Owns at most the leaf bytes; the incoming `ScalarRef` is compared
///   against it directly via [`OwnedScalar::cmp_scalar`] — no `Value` tree.
/// - `Tree` — the rare Min/Max-over-container case (Map/List/Set/Dec/Big).
///   Owns an `InnerValue` (the materialised container leaf from
///   `materialize_at`); comparison is via the generic `compare_values`
///   which is key-agnostic for scalars. Converted to `QueryValue` only
///   in `finish` (§5b boundary #3: output form).
///
/// This split keeps the hot scalar path allocation-light (a single i64 / f64
/// / small Box) while still preserving byte-identity for the rare container
/// case (which the pre-S4 path supported by holding `&'a InnerValue`).
/// Storing `InnerValue` in `Tree` avoids the need for `&Interner` in `step`.
pub(super) enum OwnedExtreme {
    Scalar(OwnedScalar),
    Tree(InnerValue),
}

/// An owned scalar leaf. Cheaper than a `Value<K>` tree: only the 6 scalar
/// arms are represented.
#[derive(Debug, Clone)]
pub(super) enum OwnedScalar {
    Null,
    Bool(bool),
    Int(i64),
    F64(f64),
    Str(Box<str>),
    Bin(Box<[u8]>),
}

impl OwnedScalar {
    /// Materialise from a borrowed `ScalarRef` (the lens leaf). This is the
    /// one justified owned-materialisation on the aggregator hot path — the
    /// running extreme must outlive the per-row borrow (§5b boundary #2).
    #[inline]
    fn from_scalar(s: ScalarRef<'_>) -> Self {
        match s {
            ScalarRef::Null => OwnedScalar::Null,
            ScalarRef::Bool(b) => OwnedScalar::Bool(b),
            ScalarRef::Int(i) => OwnedScalar::Int(i),
            ScalarRef::F64(f) => OwnedScalar::F64(f),
            ScalarRef::Str(s) => OwnedScalar::Str(s.into()),
            ScalarRef::Bin(b) => OwnedScalar::Bin(b.into()),
        }
    }

    /// Compare a borrowed incoming `ScalarRef` against the owned running
    /// extreme. Mirrors `compare_values` / `scalar_ref_cmp` arm-for-arm:
    /// Null==Null, Bool, Int/Int, **cross-type Int/F64**, F64/F64, Str/Str.
    /// Returns `None` for non-comparable pairs (mismatched families, Bin).
    #[inline]
    fn cmp_scalar(&self, incoming: ScalarRef<'_>) -> Option<Ordering> {
        // Returns `self.cmp(incoming)` (current-vs-incoming) — the call sites
        // (`cur.cmp_scalar(s)`) take the incoming for Min on `Greater` (current
        // > incoming) and for Max on `Less` (current < incoming).
        match (self, incoming) {
            (OwnedScalar::Null, ScalarRef::Null) => Some(Ordering::Equal),
            (OwnedScalar::Bool(b), ScalarRef::Bool(a)) => Some(b.cmp(&a)),
            (OwnedScalar::Int(b), ScalarRef::Int(a)) => Some(b.cmp(&a)),
            (OwnedScalar::Int(b), ScalarRef::F64(a)) => (*b as f64).partial_cmp(&a),
            (OwnedScalar::F64(b), ScalarRef::Int(a)) => b.partial_cmp(&(a as f64)),
            (OwnedScalar::F64(b), ScalarRef::F64(a)) => b.partial_cmp(&a),
            (OwnedScalar::Str(b), ScalarRef::Str(a)) => Some(b.as_ref().cmp(a)),
            // Bin and cross-family pairs are non-comparable (mirrors
            // `compare_values` returning `None`).
            _ => None,
        }
    }

    /// Materialise to the v1-output `QueryValue` form (§5b boundary #3).
    #[inline]
    fn to_query(&self) -> QueryValue {
        match self {
            OwnedScalar::Null => QueryValue::Null,
            OwnedScalar::Bool(b) => QueryValue::Bool(*b),
            OwnedScalar::Int(i) => QueryValue::Int(*i),
            OwnedScalar::F64(f) => QueryValue::F64(*f),
            OwnedScalar::Str(s) => QueryValue::Str(s.as_ref().into()),
            OwnedScalar::Bin(b) => QueryValue::Bin(b.as_ref().into()),
        }
    }
}

// ============================================================================
// Group-key fragment
// ============================================================================

/// Typed hashable key fragment used to bucket records under GROUP BY.
///
/// S4: built directly from the lens leaf (`ScalarRef`) — no `inner_to_json`
/// round-trip per record. Composite (Map/List/Set/Dec/Big) group fields stay
/// rare; they fall back to a `Box<str>` JSON canonical form materialised once
/// at the boundary (one `materialize_at` per row for that field only).
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

/// Build a `GroupKeyItem` from a borrowed `ScalarRef` (the common case — no
/// allocation beyond the leaf bytes).
#[inline]
pub(super) fn group_key_item_scalar(s: Option<ScalarRef<'_>>) -> GroupKeyItem {
    match s {
        None => GroupKeyItem::Missing,
        Some(ScalarRef::Null) => GroupKeyItem::Null,
        Some(ScalarRef::Bool(b)) => GroupKeyItem::Bool(b),
        Some(ScalarRef::Int(i)) => GroupKeyItem::Int(i),
        Some(ScalarRef::F64(f)) => GroupKeyItem::F64Bits(f.to_bits()),
        Some(ScalarRef::Str(s)) => GroupKeyItem::Str(s.into()),
        Some(ScalarRef::Bin(b)) => GroupKeyItem::Bin(b.into()),
    }
}

/// Build the complex-group-key fallback from an owned `InnerValue` (a
/// container / Dec / Big leaf materialised once via `materialize_at`). Uses
/// the JSON canonical form for parity with the pre-S4 code path.
#[inline]
pub(super) fn group_key_item_complex(val: &InnerValue, interner: &Interner) -> GroupKeyItem {
    let jv = inner_to_json_value(val, interner).unwrap_or(json::Value::Null);
    GroupKeyItem::Complex(jv.to_string().into_boxed_str())
}

// ============================================================================
// Per-aggregate accumulator
// ============================================================================

/// Per-aggregate accumulator state. One instance per `Aggregate` select item
/// in a single group. The group is walked exactly once; every record feeds
/// every accumulator via `step(record)` using the `RecordRef` lens
/// (`scalar_at` → `ScalarRef`) — no `Value` tree is materialised per row.
///
/// `Count{All}` is *not* represented here — it never touches records, so the
/// caller short-circuits to a plain `group_len` counter.
pub(super) struct AggAccum {
    /// Pre-interned field path as `InternerKey` (`None` for `AggregateField::All`).
    field_path: Option<Vec<InternerKey>>,
    /// `AggregateField::All` → step() uses the whole record as the value.
    all_field: bool,
    state: AggState,
}

pub(super) enum AggState {
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
    /// Min: owns the running extreme (§5b boundary #2). Scalar leaves use
    /// the cheap `OwnedScalar` repr; container leaves (rare) own a
    /// `QueryValue` tree.
    Min { current: Option<OwnedExtreme> },
    /// Max: owns the running extreme (§5b boundary #2).
    Max { current: Option<OwnedExtreme> },
}

impl AggAccum {
    pub(super) fn new(func: AggFunc, field: &AggregateField, interner: &Interner) -> Self {
        let (field_path, all_field) = match field {
            AggregateField::Field(p) => (intern_field_path_keys(p, interner), false),
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

    /// Resolve the aggregated value for `record` as a borrowed `ScalarRef`
    /// (the common case). Returns `None` for missing/container leaves —
    /// those are not scalars and are skipped by Sum/Avg/Count; Min/Max fall
    /// back to the container path (`resolve_owned`) when the field is
    /// genuinely a container.
    #[inline]
    fn resolve_scalar<'a, R: RecordRef + ?Sized>(&self, record: &'a R) -> Option<ScalarRef<'a>> {
        if self.all_field {
            // AggregateField::All: Count(All) is folded into the caller, so
            // the only remaining All-consumer here is a generic Agg over the
            // whole record — rare. The record itself is the "value" (always
            // present), but a record is not a scalar, so report `None` and
            // let Min/Max/Sum/Avg handle it via the owned-container path.
            None
        } else {
            self.field_path.as_deref().and_then(|p| record.scalar_at(p))
        }
    }

    #[inline]
    pub(super) fn step<R: RecordRef + ?Sized>(&mut self, record: &R) {
        let scalar = self.resolve_scalar(record);
        match &mut self.state {
            AggState::Count { count } => {
                // Count(field): count rows where the field is PRESENT (any
                // kind, including Null and containers). Pre-S4 used
                // `resolve_field_ref(..).is_some()` which counted every
                // present leaf — `present_kind_at` is the lens equivalent
                // (it reports Some for containers / Null / Dec / Big too,
                // where `scalar_at` would report None for containers).
                let present = if self.all_field {
                    true
                } else {
                    self.field_path
                        .as_deref()
                        .map(|p| record.present_kind_at(p).is_some())
                        .unwrap_or(false)
                };
                if present {
                    *count += 1;
                }
            }
            AggState::Sum {
                sum_i,
                sum_f,
                has_float,
            } => {
                if let Some(s) = scalar {
                    match s {
                        ScalarRef::Int(i) => *sum_i += i,
                        ScalarRef::F64(f) => {
                            *has_float = true;
                            *sum_f += f;
                        }
                        _ => {}
                    }
                }
            }
            AggState::Avg { sum, count } => {
                if let Some(s) = scalar {
                    match s {
                        ScalarRef::Int(i) => {
                            *sum += i as f64;
                            *count += 1;
                        }
                        ScalarRef::F64(f) => {
                            *sum += f;
                            *count += 1;
                        }
                        _ => {}
                    }
                }
            }
            AggState::Min { current } => {
                if let Some(s) = scalar {
                    // Scalar leaf — hot path. Compare directly against an
                    // OwnedScalar running extreme (no Value tree).
                    let take = match current {
                        None => true,
                        Some(OwnedExtreme::Scalar(cur)) => {
                            matches!(cur.cmp_scalar(s), Some(Ordering::Greater))
                        }
                        // Existing extreme is a container; incoming is a
                        // scalar — type-family mismatch, comparator returns
                        // None → keep the existing extreme (mirrors pre-S4).
                        Some(OwnedExtreme::Tree(_)) => false,
                    };
                    if take {
                        *current = Some(OwnedExtreme::Scalar(OwnedScalar::from_scalar(s)));
                    }
                } else {
                    // Container / Dec / Big leaf — rare Min-over-container
                    // path. Materialise this one field (§5b: justified owned
                    // boundary; one `materialize_at` per row, only here).
                    // Stored as `InnerValue` — no interner needed in `step`.
                    // Converted to QueryValue only in `finish` (§5b boundary #3).
                    // Inlined (not a `&self` method) so it borrows only
                    // `self.field_path` — disjoint from the `&mut self.state`
                    // match in scope. §5b: one `materialize_at` per row, only
                    // on the rare Min/Max-over-container leaf.
                    let owned = if self.all_field {
                        None
                    } else {
                        self.field_path
                            .as_deref()
                            .and_then(|p| record.materialize_at(p))
                    };
                    if let Some(v) = owned {
                        let take = match current {
                            None => true,
                            Some(OwnedExtreme::Tree(cur)) => {
                                // compare_values<K> is key-agnostic on scalars;
                                // container comparisons return None → keep existing.
                                matches!(compare_values(cur, &v), Some(Ordering::Greater))
                            }
                            // Existing is a scalar; incoming is a container
                            // → mismatch → keep existing.
                            Some(OwnedExtreme::Scalar(_)) => false,
                        };
                        if take {
                            *current = Some(OwnedExtreme::Tree(v));
                        }
                    }
                }
            }
            AggState::Max { current } => {
                if let Some(s) = scalar {
                    let take = match current {
                        None => true,
                        Some(OwnedExtreme::Scalar(cur)) => {
                            matches!(cur.cmp_scalar(s), Some(Ordering::Less))
                        }
                        Some(OwnedExtreme::Tree(_)) => false,
                    };
                    if take {
                        *current = Some(OwnedExtreme::Scalar(OwnedScalar::from_scalar(s)));
                    }
                } else {
                    // Inlined (not a `&self` method) so it borrows only
                    // `self.field_path` — disjoint from the `&mut self.state`
                    // match in scope. §5b: one `materialize_at` per row, only
                    // on the rare Min/Max-over-container leaf.
                    let owned = if self.all_field {
                        None
                    } else {
                        self.field_path
                            .as_deref()
                            .and_then(|p| record.materialize_at(p))
                    };
                    if let Some(v) = owned {
                        let take = match current {
                            None => true,
                            Some(OwnedExtreme::Tree(cur)) => {
                                matches!(compare_values(cur, &v), Some(Ordering::Less))
                            }
                            Some(OwnedExtreme::Scalar(_)) => false,
                        };
                        if take {
                            *current = Some(OwnedExtreme::Tree(v));
                        }
                    }
                }
            }
        }
    }

    pub(super) fn finish(self, interner: &Interner) -> QueryValue {
        match self.state {
            AggState::Count { count } => QueryValue::Int(count as i64),
            AggState::Sum {
                sum_i,
                sum_f,
                has_float,
            } => {
                if has_float {
                    let total = sum_f + sum_i as f64;
                    // Match json Number::from_f64 semantics: NaN/Inf → Null.
                    if total.is_finite() {
                        QueryValue::F64(total)
                    } else {
                        QueryValue::Null
                    }
                } else {
                    QueryValue::Int(sum_i)
                }
            }
            AggState::Avg { sum, count } => {
                if count == 0 {
                    QueryValue::Null
                } else {
                    let avg = sum / count as f64;
                    if avg.is_finite() {
                        QueryValue::F64(avg)
                    } else {
                        QueryValue::Null
                    }
                }
            }
            AggState::Min { current } | AggState::Max { current } => current
                .map(|c| match c {
                    OwnedExtreme::Scalar(s) => s.to_query(),
                    // Tree holds InnerValue (no interner needed in step).
                    // Convert at output boundary only (§5b boundary #3).
                    OwnedExtreme::Tree(iv) => {
                        inner_value_to_query_value(&iv, interner).unwrap_or(QueryValue::Null)
                    }
                })
                .unwrap_or(QueryValue::Null),
        }
    }
}

// ============================================================================
// Small helpers — keep the impl above readable.
// ============================================================================

/// Intern a `&[String]` path into `InternerKey`s (the lens path currency).
/// `None` on any un-internable segment (mirrors `intern_field_path`'s old
/// `Option<Vec<u64>>` miss).
#[inline]
fn intern_field_path_keys(field: &[String], interner: &Interner) -> Option<Vec<InternerKey>> {
    let mut keys = Vec::with_capacity(field.len());
    for part in field {
        let id = interner.get_ind(part)?;
        keys.push(id);
    }
    Some(keys)
}

// ============================================================================
// build_aggregate_object
// ============================================================================

/// Build a JSON object from select items for a group of records.
///
/// S4: the group is walked as `&[(RecordId, Bytes)]`; a per-row `RecordView`
/// (with a bare-scalar `InnerValue::from_bytes` fallback) supplies every
/// accumulator via the `RecordRef` lens. `SelectItem::CountAll` is
/// short-circuited to `group_records.len()` (no lens touched).
pub(super) fn build_aggregate_object(
    group_records: &[(RecordId, Bytes)],
    select: &Select,
    group_key_values: Option<&[(String, QueryValue)]>,
    interner: &Interner,
) -> QueryValue {
    let mut obj: indexmap::IndexMap<String, QueryValue, shamir_collections::THasher> =
        new_map_wc(select.items.len());

    // Add group key values if provided
    if let Some(keys) = group_key_values {
        for (key, val) in keys {
            obj.insert(key.clone(), val.clone());
        }
    }

    // First pass over select items: allocate accumulators and remember the
    // output key for each one.
    let mut agg_slots: Vec<(String, AggAccum)> = Vec::new();
    let mut field_slots: Vec<(String, &[String])> = Vec::new();
    #[allow(clippy::type_complexity)] // parallel to agg_slots; clarity over brevity
    let mut fn_slots: Vec<(
        String,
        Option<Vec<InternerKey>>,
        bool,
        Option<Box<dyn Aggregator>>,
    )> = Vec::new();
    let mut func_slots: Vec<(String, FilterValue)> = Vec::new();

    for item in &select.items {
        match item {
            SelectItem::CountAll { alias } => {
                let key = alias.as_deref().unwrap_or("count");
                obj.insert(key.to_string(), QueryValue::Int(group_records.len() as i64));
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
                    AggregateField::Field(p) => (intern_field_path_keys(p, interner), false),
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

    // Single walk over the group: build a per-row `RecordRef` (RecordView on
    // the hot path, InnerValue on the bare-scalar fallback) and feed every
    // accumulator + funclib slot at once.
    if !agg_slots.is_empty() || !fn_slots.is_empty() {
        for (_, bytes) in group_records {
            // Per-row lens. RecordView::new fails only for bare-scalar /
            // non-map records (legacy rows) — fall back to InnerValue, which
            // is also a RecordRef. Both arms share the same accumulator
            // surface (scalar_at / materialize_at).
            let view: RowView = match RecordView::new(bytes) {
                Ok(v) => RowView::Lens(v),
                Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                    Ok(iv) => RowView::Tree(iv),
                    // Malformed row — skip defensively (mirrors the read
                    // branches' behaviour).
                    Err(_) => continue,
                },
            };

            for (_, acc) in agg_slots.iter_mut() {
                view.with_ref(|r| acc.step(r));
            }
            for (_, path, all_field, agg) in fn_slots.iter_mut() {
                if let Some(agg) = agg {
                    let qv =
                        view.with_ref(|r| fn_value_for_aggregator(r, path, *all_field, interner));
                    if let Some(qv) = qv {
                        let _ = agg.accumulate(&qv);
                    }
                }
            }
        }
    }

    for (key, acc) in agg_slots {
        obj.insert(key, acc.finish(interner));
    }

    for (key, _, _, agg) in fn_slots {
        let qv = match agg {
            Some(agg) => match agg.finalize() {
                Ok(v) => v,
                Err(_) => QueryValue::Null,
            },
            None => QueryValue::Null,
        };
        obj.insert(key, qv);
    }

    // Resolve field-projection fallbacks from the first record (rare path).
    for (key, path) in field_slots {
        let val = group_records
            .first()
            .and_then(|(_, bytes)| {
                let keys = intern_field_path_keys(path, interner)?;
                match RecordView::new(bytes) {
                    Ok(v) => v
                        .scalar_at(keys.as_slice())
                        .map(scalar_ref_to_query)
                        .or_else(|| {
                            // Container leaf — materialise once.
                            v.materialize_at(keys.as_slice())
                                .and_then(|iv| inner_value_to_query_value(&iv, interner).ok())
                        }),
                    Err(_) => InnerValue::from_bytes(bytes.as_ref()).ok().and_then(|iv| {
                        iv.scalar_at(keys.as_slice())
                            .map(scalar_ref_to_query)
                            .or_else(|| {
                                iv.materialize_at(keys.as_slice())
                                    .and_then(|v| inner_value_to_query_value(&v, interner).ok())
                            })
                    }),
                }
            })
            .unwrap_or(QueryValue::Null);
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
                .and_then(|(_, bytes)| {
                    let view = match RecordView::new(bytes) {
                        Ok(v) => RowView::Lens(v),
                        Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                            Ok(iv) => RowView::Tree(iv),
                            Err(_) => return None,
                        },
                    };
                    view.with_ref(|r| resolve_filter_value(&fv, r, &ctx))
                        .map(|v| {
                            inner_value_to_query_value(&v, interner).unwrap_or(QueryValue::Null)
                        })
                })
                .unwrap_or(QueryValue::Null);
            obj.insert(key, val);
        }
    }

    QueryValue::Map(obj)
}

/// Per-row view carrier: a small enum that abstracts over `RecordView` (hot)
/// and `InnerValue` (bare-scalar fallback) so the accumulator loop can be
/// written once. Both arms implement `RecordRef`.
enum RowView<'a> {
    Lens(RecordView<'a>),
    Tree(InnerValue),
}

impl<'a> RowView<'a> {
    /// Apply `f` to the underlying `RecordRef`.
    ///
    /// `v: &'a RecordView<'a>` (via match ergonomics on `&'a self`) coerces
    /// to `&'a dyn RecordRef`; `t: &'a InnerValue` coerces likewise.
    /// Both avoid dereferencing to a value type (RecordView is Copy but &T →
    /// &dyn Trait requires a reference; InnerValue is not Copy so `*t` would
    /// move out from behind a reference).
    #[inline]
    fn with_ref<R, O>(&'a self, f: R) -> O
    where
        R: FnOnce(&dyn RecordRef) -> O,
    {
        match self {
            RowView::Lens(v) => f(v),
            RowView::Tree(t) => f(t),
        }
    }
}

/// Build the `QueryValue` argument for a funclib aggregator at the funclib
/// boundary only. Scalars are constructed directly from `ScalarRef` (cheap);
/// container leaves (and `AggregateField::All`) materialise ONCE via
/// `materialize_at` → `inner_value_to_query_value`. This is the §5b boundary
/// #1 (funclib ABI takes owned) — there is no whole-record conversion.
#[inline]
fn fn_value_for_aggregator(
    record: &dyn RecordRef,
    path: &Option<Vec<InternerKey>>,
    all_field: bool,
    interner: &Interner,
) -> Option<QueryValue> {
    if all_field {
        // AggregateField::All: the whole record is the value. Build the
        // QueryValue via the lens's `to_query_value` (single boundary).
        return Some(record.to_query_value(interner));
    }
    let path = path.as_deref()?;
    // Try scalar first (the hot case): no Value materialisation.
    if let Some(s) = record.scalar_at(path) {
        return Some(scalar_ref_to_query(s));
    }
    // Container / Dec / Big leaf — materialise this one field only.
    record
        .materialize_at(path)
        .and_then(|iv| inner_value_to_query_value(&iv, interner).ok())
}

/// Convert a `ScalarRef` to a `QueryValue` (the only leaf-level conversion;
/// cheap — no interner touch). Used at the funclib feed boundary and for
/// field-projection fallbacks.
#[inline]
fn scalar_ref_to_query(s: ScalarRef<'_>) -> QueryValue {
    match s {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.into()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.into()),
    }
}

// ============================================================================
// Group By
// ============================================================================

/// Apply GROUP BY + aggregation + HAVING.
pub fn apply_group_by(
    records: &[(RecordId, Bytes)],
    group_by: &GroupBy,
    select: &Select,
    interner: &Interner,
    ctx: &FilterContext<'_>,
) -> Vec<QueryValue> {
    if records.is_empty() {
        return Vec::new();
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

    for (id, bytes) in records {
        // Per-row lens for the group-key resolution (same hot/fallback pair
        // as the accumulator loop). The bytes are refcounted `Bytes` —
        // cheap to clone into the group bucket.
        let key_items: Vec<GroupKeyItem> = match RecordView::new(bytes) {
            Ok(v) => group_paths
                .iter()
                .map(|(_, path)| group_key_from_lens(&v, path.as_deref(), interner))
                .collect(),
            Err(_) => match InnerValue::from_bytes(bytes.as_ref()) {
                Ok(iv) => group_paths
                    .iter()
                    .map(|(_, path)| group_key_from_lens(&iv, path.as_deref(), interner))
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
        ));
    }

    // Apply HAVING filter — S4: compare via the `QueryValue` result map
    // directly (no `query_value_to_inner` bridge). `HavingView` is a thin
    // `RecordRef` adapter that re-keys String → `InternerKey` once and
    // serves leaves as `ScalarRef` straight off the `QueryValue`.
    if let Some(having_filter) = &group_by.having {
        pre_intern_select_keys(select, interner);
        let having_cb = compile_filter(having_filter, interner);
        result.retain(|qv| {
            let view = HavingView::new(qv, interner);
            having_cb.matches(&view, ctx)
        });
    }

    result
}

/// Resolve a single group-by field to a `GroupKeyItem` from a `RecordRef`
/// lens. Scalar leaves go through the lens directly; container / Dec / Big
/// leaves fall back to one `materialize_at` + JSON canonical form (rare).
#[inline]
fn group_key_from_lens<R: RecordRef + ?Sized>(
    record: &R,
    path: Option<&[InternerKey]>,
    interner: &Interner,
) -> GroupKeyItem {
    let Some(path) = path else {
        return GroupKeyItem::Missing;
    };
    if let Some(s) = record.scalar_at(path) {
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
) -> Vec<QueryValue> {
    let obj = build_aggregate_object(records, select, None, interner);
    vec![obj]
}
