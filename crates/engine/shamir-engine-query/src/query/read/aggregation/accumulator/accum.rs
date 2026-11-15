//! The per-aggregate accumulator (`AggAccum`) and its numeric helpers.

use std::cmp::Ordering;

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;

use crate::query::filter::eval::compare_values;
use crate::query::filter::numeric_cmp::cmp_i64_f64;
use crate::query::read::{AggFunc, AggregateField};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::codecs::interned::{
    inner_value_to_query_value, inner_value_to_query_value_with, query_value_to_inner,
};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::{InnerValue, QueryValue};

use super::extreme::{OwnedExtreme, OwnedScalar};

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
pub(in crate::query::read) struct AggAccum {
    /// Pre-interned field path as `InternerKey` (`None` for `AggregateField::All`).
    field_path: Option<Vec<InternerKey>>,
    /// Overlay reverse snapshot used only when converting owned container extrema.
    overlay: Option<std::sync::Arc<shamir_collections::TFxMap<u64, String>>>,
    /// `AggregateField::All` → step() uses the whole record as the value.
    all_field: bool,
    pub(in crate::query::read::aggregation) state: AggState,
}

pub(in crate::query::read) enum AggState {
    /// Count(field) → non-null count.  Count(All) is folded into the caller.
    Count { count: u64 },
    /// Sum: `i64` fast path (`sum_i`), lifted EXACTLY to `sum_big` once a
    /// `Big` value or an `i64` overflow is seen (still no precision loss —
    /// see `sum_big`'s doc), and lifted to the (lossy) `f64` lane only once
    /// a genuinely non-integer value (`F64`, or a `Dec` with a fractional
    /// part) enters the same aggregate — CR-C5 (#780). That last transition
    /// is a one-way door: once `has_float` is set, the aggregate stays
    /// float-based for the rest of the group (mixing an exact running total
    /// with a float is unavoidably lossy from that point on; this is a
    /// documented, deliberate transition, not a bug).
    Sum {
        sum_i: i64,
        /// Exact overflow/`Big` lane. `None` until the first `Big` value or
        /// `sum_i` overflow is observed; once `Some`, it (not `sum_i`) is
        /// the authoritative exact running total — `sum_i` stops being
        /// added to once this is created (see `step`'s `Sum` arm).
        sum_big: Option<BigInt>,
        sum_f: f64,
        has_float: bool,
    },
    /// Avg: f64 accumulator + non-null numeric count.
    ///
    /// CR-C5 (#780): unlike `Sum`, `Avg`'s intermediate running total stays
    /// `f64`-based even for pure-integer/`Big` inputs — deliberate, per the
    /// task's own guidance ("Avg inherently produces a non-integer result;
    /// the final division stays f64 regardless"). An exact-`BigInt`
    /// intermediate would still have to divide down to a `QueryValue::F64`
    /// at `finish()`, so it buys no precision at the only point precision is
    /// ever observed — restructuring it here would be scope creep beyond
    /// this task's Sum-focused exactness fix. `Big` inputs to Avg go through
    /// the existing `agg_leaf_to_f64` fallback unchanged (still lossy,
    /// tracked, matches `agg_leaf_to_f64`'s doc comment).
    Avg { sum: f64, count: u64 },
    /// Min: owns the running extreme (§5b boundary #2). Scalar leaves use
    /// the cheap `OwnedScalar` repr; container leaves (rare) own a
    /// `QueryValue` tree.
    Min { current: Option<OwnedExtreme> },
    /// Max: owns the running extreme (§5b boundary #2).
    Max { current: Option<OwnedExtreme> },
}

impl AggAccum {
    #[cfg(test)]
    pub(in crate::query::read) fn new(
        func: AggFunc,
        field: &AggregateField,
        interner: &Interner,
    ) -> Self {
        Self::new_with_snapshots(func, field, interner, None, None)
    }

    pub(in crate::query::read) fn new_with_snapshots(
        func: AggFunc,
        field: &AggregateField,
        interner: &Interner,
        overlay_ids: Option<&shamir_collections::TFxMap<String, u64>>,
        overlay: Option<std::sync::Arc<shamir_collections::TFxMap<u64, String>>>,
    ) -> Self {
        let (field_path, all_field) = match field {
            AggregateField::Field(p) => (
                intern_field_path_keys_with_overlay(p, interner, overlay_ids),
                false,
            ),
            AggregateField::All => (None, true),
        };
        let state = match func {
            AggFunc::Count => AggState::Count { count: 0 },
            AggFunc::Sum => AggState::Sum {
                sum_i: 0,
                sum_big: None,
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
            overlay,
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
    pub(in crate::query::read) fn step<R: RecordRef + ?Sized>(&mut self, record: &R) {
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
                sum_big,
                sum_f,
                has_float,
            } => {
                if let Some(s) = scalar {
                    match s {
                        ScalarRef::Int(i) => {
                            if *has_float {
                                *sum_f += i as f64;
                            } else if let Some(big) = sum_big {
                                // Exact lane already active (a prior Big
                                // value or i64 overflow promoted it) — fold
                                // this Int into it exactly, `sum_i` is no
                                // longer touched once `sum_big` exists.
                                *big += i;
                            } else {
                                match sum_i.checked_add(i) {
                                    Some(new_sum) => *sum_i = new_sum,
                                    None => {
                                        // i64 overflow — promote to the exact
                                        // BigInt lane (CR-C5, #780), NOT the
                                        // lossy float lane: `sum_i + i` is
                                        // still a genuine integer, it just no
                                        // longer fits in i64.
                                        *sum_big = Some(BigInt::from(*sum_i) + i);
                                    }
                                }
                            }
                        }
                        ScalarRef::F64(f) => {
                            // Genuine float enters the aggregate — one-way
                            // transition to float semantics (documented on
                            // `AggState::Sum`). Fold whatever exact total we
                            // have so far into `sum_f` before switching.
                            if !*has_float {
                                *sum_f += sum_big
                                    .take()
                                    .map(|b| b.to_f64().unwrap_or(f64::NAN))
                                    .unwrap_or(*sum_i as f64);
                                *has_float = true;
                            }
                            *sum_f += f;
                        }
                        _ => {}
                    }
                } else {
                    // Dec/Big/container fallback — materialize this one
                    // field (§5b: one `materialize_at` per row, only when
                    // `scalar_at` returned None, i.e. the field is Dec/Big/
                    // container). CR-C5 (#780): `Big` and integer-valued
                    // `Dec` values fold into the EXACT `sum_big` lane;
                    // `Dec` with a genuine fractional part is a real
                    // non-integer value and transitions the whole aggregate
                    // to the (documented, unavoidable) float lane, same as
                    // an `F64` row above. Containers (Map/List/Set)
                    // contribute nothing (unchanged).
                    let owned = if self.all_field {
                        None
                    } else {
                        self.field_path
                            .as_deref()
                            .and_then(|p| record.materialize_at(p))
                    };
                    if let Some(v) = owned {
                        match agg_leaf_exact(&v) {
                            Some(AggLeafExact::Int(exact)) if !*has_float => match sum_big {
                                Some(big) => *big += exact,
                                None => *sum_big = Some(BigInt::from(*sum_i) + exact),
                            },
                            Some(AggLeafExact::Int(exact)) => {
                                // Already float mode — fold exactly-known
                                // integer value in as f64 (matches the
                                // existing lossy-once-float semantics).
                                *sum_f += exact.to_f64().unwrap_or(f64::NAN);
                            }
                            None => {
                                if let Some(f) = agg_leaf_to_f64(&v) {
                                    if !*has_float {
                                        *sum_f += sum_big
                                            .take()
                                            .map(|b| b.to_f64().unwrap_or(f64::NAN))
                                            .unwrap_or(*sum_i as f64);
                                        *has_float = true;
                                    }
                                    *sum_f += f;
                                }
                            }
                        }
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
                } else {
                    // Dec/Big/container fallback. CR-C5 (#780): UNLIKE Sum,
                    // Avg's running total stays `f64`-based here on purpose
                    // — see `AggState::Avg`'s doc comment for the reasoning
                    // (the final division is always `f64` anyway, so an
                    // exact intermediate buys nothing at the only point
                    // precision is observed). Dec/Big values are converted
                    // to f64 and flow into the same accumulator lane as F64
                    // (documented, accepted precision-loss tradeoff).
                    let owned = if self.all_field {
                        None
                    } else {
                        self.field_path
                            .as_deref()
                            .and_then(|p| record.materialize_at(p))
                    };
                    if let Some(v) = owned {
                        if let Some(f) = agg_leaf_to_f64(&v) {
                            *sum += f;
                            *count += 1;
                        }
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

    pub(in crate::query::read) fn finish(self, interner: &Interner) -> QueryValue {
        match self.state {
            AggState::Count { count } => QueryValue::Int(count as i64),
            AggState::Sum {
                sum_i,
                sum_big,
                sum_f,
                has_float,
            } => {
                if has_float {
                    // `sum_big`/`sum_i` were already folded into `sum_f` at
                    // the point of transition (see `step`'s `Sum` arm) —
                    // `sum_f` alone is the running total once float mode is
                    // active.
                    if sum_f.is_finite() {
                        QueryValue::F64(sum_f)
                    } else {
                        // NaN/Inf → Null (non-finite float result).
                        QueryValue::Null
                    }
                } else if let Some(big) = sum_big {
                    // CR-C5 (#780): exact `BigInt` total — a `Big` value or
                    // an `i64` overflow was seen, but no genuine float/
                    // fractional value entered the aggregate, so the result
                    // is exact. Encode as `QueryValue::Big` (the same
                    // representation the storage layer already uses for a
                    // single out-of-`i64`-range value — FG-1/FG-6) rather
                    // than lossily downcasting through `f64`.
                    QueryValue::Big(big)
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
                    OwnedExtreme::Tree(iv) => match &self.overlay {
                        Some(overlay) => inner_value_to_query_value_with(&iv, &|id| {
                            if id >= shamir_tx::OVERLAY_ID_BASE {
                                overlay.get(&id).cloned()
                            } else {
                                interner
                                    .get_str(&InternerKey::new(id))
                                    .map(|name| name.to_string())
                            }
                        })
                        .unwrap_or(QueryValue::Null),
                        None => {
                            inner_value_to_query_value(&iv, interner).unwrap_or(QueryValue::Null)
                        }
                    },
                })
                .unwrap_or(QueryValue::Null),
        }
    }
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
            // Int<->F64: F-2 (#791), exact via the shared
            // `numeric_cmp::cmp_i64_f64` — replaces the lossy `*b as f64`/
            // `a as f64` casts, which could round a large `i64` running
            // extreme (or incoming value) to a DIFFERENT `f64` than what it
            // actually represents (same class of bug as W-1/CR-D3).
            (OwnedScalar::Int(b), ScalarRef::F64(a)) => cmp_i64_f64(*b, a),
            (OwnedScalar::F64(b), ScalarRef::Int(a)) => cmp_i64_f64(a, *b).map(Ordering::reverse),
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
// Small helpers — keep the impl above readable.
// ============================================================================

/// Extract a numeric `f64` from a materialised `InnerValue` leaf for the
/// Sum/Avg container fallback. Only `Dec`/`Big` produce a value; all other
/// leaves (containers, Null, Str, Bool, Bin) return `None` and contribute
/// nothing — matching the pre-existing behaviour where non-numeric fields
/// were silently skipped.
///
/// CR-C5 (#780): `Avg`'s intermediate running total is deliberately kept
/// `f64`-based for `Big`/non-integer-`Dec` inputs (see `AggState::Avg`'s doc
/// comment) — this helper remains its accepted, documented precision-loss
/// tradeoff. `Sum`, however, no longer calls this for `Big`/integer-`Dec`
/// inputs while it is still in exact mode — see [`agg_leaf_exact`] and
/// `AggAccum::step`'s `Sum` arm, which use this only once the aggregate has
/// already transitioned to float mode (at which point exactness is moot).
#[inline]
fn agg_leaf_to_f64(v: &InnerValue) -> Option<f64> {
    match v {
        InnerValue::Dec(d) => d.to_f64(),
        InnerValue::Big(b) => Some(b.to_f64().unwrap_or(f64::NAN)),
        _ => None,
    }
}

/// Result of [`agg_leaf_exact`]: an `InnerValue` leaf that carries an EXACT
/// integer value (no precision loss), vs one that does not (genuinely
/// fractional `Dec`, or a non-numeric leaf).
enum AggLeafExact {
    /// The leaf is an exact integer — `Big` directly, or a `Dec` whose
    /// fractional part is zero (e.g. `100.00`).
    Int(BigInt),
}

/// Extract an EXACT integer value from a materialised `InnerValue` leaf, for
/// `Sum`'s exact-`BigInt` accumulation lane — CR-C5 (#780).
///
/// - `Big` is always exact (arbitrary-precision integer by construction).
/// - `Dec` is exact ONLY when its fractional part is zero — a `Dec` with a
///   genuine fractional part (e.g. `1.5`) is not representable as an
///   integer without loss, so it returns `None` and the caller falls back
///   to the (documented) float lane instead of silently truncating.
/// - Everything else (containers, Null, Str, Bool, Bin) returns `None`,
///   same as `agg_leaf_to_f64`.
#[inline]
fn agg_leaf_exact(v: &InnerValue) -> Option<AggLeafExact> {
    match v {
        InnerValue::Big(b) => Some(AggLeafExact::Int(b.clone())),
        InnerValue::Dec(d) if d.fract().is_zero() => {
            // `Decimal::trunc()`'s mantissa/scale-0 form round-trips through
            // `BigInt` exactly for integer-valued Decimals within its 96-bit
            // range — no `f64` involved.
            Some(AggLeafExact::Int(BigInt::from(d.trunc().mantissa())))
        }
        _ => None,
    }
}

/// Intern a `&[String]` path into `InternerKey`s; `None` on any un-internable segment.
pub(in crate::query::read::aggregation) fn intern_field_path_keys_with_overlay(
    field: &[String],
    interner: &Interner,
    overlay_ids: Option<&shamir_collections::TFxMap<String, u64>>,
) -> Option<Vec<InternerKey>> {
    let mut keys = Vec::with_capacity(field.len());
    for part in field {
        let id = interner.get_ind(part).or_else(|| {
            overlay_ids
                .and_then(|ids| ids.get(part).copied())
                .map(InternerKey::new)
        })?;
        keys.push(id);
    }
    Some(keys)
}

// ============================================================================
// Aggregate-select validation (Fix 1b + Fix 2b)
// ============================================================================

/// Expression NULL results are skipped by all aggregates, including COUNT.
/// Empty SUM/COUNT return 0; empty AVG/MIN/MAX return NULL. Literal NULL
/// SUM is finalized as NULL by build_aggregate_object. Legacy field
/// behaviour is unchanged: COUNT(field) counts present NULLs, and MIN/MAX
/// can retain a NULL extreme. Expression SUM/AVG require numeric values;
/// MIN/MAX require comparable values (including comparable numeric families).
pub(in crate::query::read::aggregation) fn step_aggregate_input(
    acc: &mut AggAccum,
    value: &QueryValue,
    interner: &Interner,
) -> DbResult<()> {
    if matches!(value, QueryValue::Null) {
        return Ok(());
    }
    let leaf = query_value_to_inner(value, interner)
        .map_err(|error| DbError::Validation(error.to_string()))?;
    match &mut acc.state {
        AggState::Sum { .. } | AggState::Avg { .. } => {
            if !matches!(
                value,
                QueryValue::Int(_) | QueryValue::F64(_) | QueryValue::Dec(_) | QueryValue::Big(_)
            ) {
                return Err(DbError::Validation("type_mismatch".into()));
            }
        }
        AggState::Min { current } => {
            let ordering = match current {
                Some(OwnedExtreme::Tree(cur)) => compare_values(cur, &leaf),
                None => compare_values(&leaf, &leaf),
                Some(OwnedExtreme::Scalar(_)) => unreachable!("expression extrema use leaves"),
            }
            .ok_or_else(|| DbError::Validation("type_mismatch".into()))?;
            if current.is_none() || ordering == Ordering::Greater {
                *current = Some(OwnedExtreme::Tree(leaf));
            }
            return Ok(());
        }
        AggState::Max { current } => {
            let ordering = match current {
                Some(OwnedExtreme::Tree(cur)) => compare_values(cur, &leaf),
                None => compare_values(&leaf, &leaf),
                Some(OwnedExtreme::Scalar(_)) => unreachable!("expression extrema use leaves"),
            }
            .ok_or_else(|| DbError::Validation("type_mismatch".into()))?;
            if current.is_none() || ordering == Ordering::Less {
                *current = Some(OwnedExtreme::Tree(leaf));
            }
            return Ok(());
        }
        AggState::Count { .. } => {}
    }
    // RecordRef rejects empty paths: wrap the evaluated leaf in a one-field record.
    // RU8: keep the typed quota error (wire code interner_quota_exceeded)
    // instead of flattening it into a Validation string.
    let key = interner
        .touch_ind("agg_input")
        .map_err(shamir_storage::error::DbError::from)?
        .into_key();
    acc.field_path = Some(vec![key.clone()]);
    acc.all_field = false;
    let mut record = new_map_wc(1);
    record.insert(key, leaf);
    acc.step(&InnerValue::Map(record));
    Ok(())
}
