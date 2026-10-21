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

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;

use crate::query::filter::eval::{compare_values, resolve_filter_query};
use crate::query::filter::numeric_cmp::cmp_i64_f64;
use crate::query::filter::{FilterContext, FilterValue};
use crate::query::read::{AggFunc, AggregateField};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::codecs::interned::{inner_value_to_query_value, query_value_to_inner};
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::record_view::{RecordRef, ScalarRef};
use shamir_types::types::common::new_map_wc;
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
pub(in crate::query::read) enum OwnedExtreme {
    Scalar(OwnedScalar),
    Tree(InnerValue),
}

/// An owned scalar leaf. Cheaper than a `Value<K>` tree: only the 6 scalar
/// arms are represented.
#[derive(Debug, Clone)]
pub(in crate::query::read) enum OwnedScalar {
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
// Group-key fragment
// ============================================================================

/// Typed hashable key fragment used to bucket records under GROUP BY.
///
/// S4: built directly from the lens leaf (`ScalarRef`) — no extra
/// round-trip per record. Composite (Map/List/Set/Dec/Big) group fields stay
/// rare; they fall back to a `Box<str>` canonical form materialised once
/// at the boundary (one `materialize_at` per row for that field only).
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::query::read) enum GroupKeyItem {
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
pub(in crate::query::read) fn group_key_item_scalar(s: Option<ScalarRef<'_>>) -> GroupKeyItem {
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
/// container / Dec / Big leaf materialised once via `materialize_at`).
///
/// Converts `InnerValue` → `QueryValue` via the interner, then serialises to
/// a canonical string that matches the equivalence classes of the old
/// string-keyed path — preserving grouping behaviour.
#[inline]
pub(in crate::query::read) fn group_key_item_complex(val: &InnerValue, interner: &Interner) -> GroupKeyItem {
    let qv = inner_value_to_query_value(val, interner).unwrap_or(QueryValue::Null);
    GroupKeyItem::Complex(query_value_canonical_str(&qv).into_boxed_str())
}

/// Produce a canonical string for a `QueryValue` that matches the equivalence
/// classes of the pre-J3 code path.
///
/// This gives the same GROUP BY equivalence classes as the pre-J3 code path,
/// with no external allocations.
fn query_value_canonical_str(qv: &QueryValue) -> String {
    match qv {
        QueryValue::Null => "null".to_string(),
        QueryValue::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        QueryValue::Int(i) => i.to_string(),
        QueryValue::F64(f) => {
            if f.is_finite() {
                // Finite f64: use Rust's default float formatter which
                // uses Grisu/Dragon and produces the shortest round-trip
                // representation.
                format!("{f}")
            } else {
                // Non-finite f64 becomes a quoted string in the old path.
                format!("\"{}\"", f)
            }
        }
        QueryValue::Dec(d) => format!("\"{}\"", d),
        QueryValue::Big(b) => format!("\"{}\"", b),
        QueryValue::Str(s) => {
            // Escape the string: backslash and double-quote are the only
            // characters that must be escaped for the ASCII content that
            // appears in group keys. Full unicode escaping is not needed —
            // non-ASCII chars are written as UTF-8 (no \uXXXX).
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        // Control characters: \uXXXX
                        let _ = std::fmt::Write::write_fmt(
                            &mut out,
                            format_args!("\\u{:04x}", c as u32),
                        );
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
        QueryValue::Bin(bytes) => {
            // Bin → canonical Array([byte as i64, ...]) representation.
            let mut out = String::with_capacity(bytes.len() * 3 + 2);
            out.push('[');
            for (i, &b) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{}", b));
            }
            out.push(']');
            out
        }
        QueryValue::List(l) => {
            let mut out = String::from("[");
            for (i, item) in l.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(item));
            }
            out.push(']');
            out
        }
        QueryValue::Set(s) => {
            let mut out = String::from("[");
            for (i, item) in s.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(item));
            }
            out.push(']');
            out
        }
        QueryValue::Map(m) => {
            let mut out = String::from("{");
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&query_value_canonical_str(&QueryValue::Str(k.clone())));
                out.push(':');
                out.push_str(&query_value_canonical_str(v));
            }
            out.push('}');
            out
        }
    }
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
pub(in crate::query::read) struct AggAccum {
    /// Pre-interned field path as `InternerKey` (`None` for `AggregateField::All`).
    field_path: Option<Vec<InternerKey>>,
    /// `AggregateField::All` → step() uses the whole record as the value.
    all_field: bool,
    pub(super) state: AggState,
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
    pub(in crate::query::read) fn new(func: AggFunc, field: &AggregateField, interner: &Interner) -> Self {
        let (field_path, all_field) = match field {
            AggregateField::Field(p) => (intern_field_path_keys(p, interner), false),
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

/// Intern a `&[String]` path into `InternerKey`s (the lens path currency).
/// `None` on any un-internable segment (mirrors `intern_field_path`'s old
/// `Option<Vec<u64>>` miss).
#[inline]
pub(super) fn intern_field_path_keys(field: &[String], interner: &Interner) -> Option<Vec<InternerKey>> {
    let mut keys = Vec::with_capacity(field.len());
    for part in field {
        let id = interner.get_ind(part)?;
        keys.push(id);
    }
    Some(keys)
}

// ============================================================================
// Aggregate-select validation (Fix 1b + Fix 2b)
// ============================================================================

/// Whether a [`FilterValue`] is a literal. Dynamic execution constants are
/// validated after binding, while literal type/range errors are caught early.
pub(super) fn is_literal(fv: &FilterValue) -> bool {
    matches!(
        fv,
        FilterValue::Null
            | FilterValue::Bool(_)
            | FilterValue::Int(_)
            | FilterValue::Float(_)
            | FilterValue::String(_)
            | FilterValue::Binary(_)
            | FilterValue::Array(_)
    )
}

/// Factory configuration may depend on this execution, but never on a row.
/// Conditionals and function calls are conservatively excluded: they can carry
/// implicit row inputs. AggregateFn.args is not the aggregate's row expression.
pub(super) fn is_execution_constant(fv: &FilterValue) -> bool {
    match fv {
        FilterValue::FieldRef { .. } | FilterValue::Cond { .. } | FilterValue::FnCall { .. } => {
            false
        }
        FilterValue::Array(values) => values.iter().all(is_execution_constant),
        FilterValue::Expr { expr } => expr.args.iter().all(is_execution_constant),
        _ => true,
    }
}

/// Resolve with an empty row and a fresh execution-local context. Preflight
/// children so the evaluator's Null fallback cannot hide an unresolved binding.
pub(super) fn resolve_aggregate_param(fv: &FilterValue, ctx: &FilterContext<'_>) -> DbResult<QueryValue> {
    match fv {
        FilterValue::Expr { expr } => {
            for arg in &expr.args {
                resolve_aggregate_param(arg, ctx)?;
            }
        }
        FilterValue::Array(values) => {
            for value in values {
                resolve_aggregate_param(value, ctx)?;
            }
        }
        _ => {}
    }
    let fresh = FilterContext::new(ctx.interner, ctx.resolved_refs)
        .with_params(ctx.params)
        .with_scalars(ctx.scalars.clone());
    resolve_filter_query(fv, &InnerValue::Null, &fresh)
        .ok_or_else(|| DbError::Validation("agg_param_unresolved".into()))
}

/// Extract a literal `f64` from a [`FilterValue`] (`Float` or `Int`).
pub(super) fn literal_f64(fv: &FilterValue) -> Option<f64> {
    match fv {
        FilterValue::Float(f) => Some(*f),
        FilterValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// Extract a literal `&str` from a [`FilterValue`] (`String`).
pub(super) fn literal_string(fv: &FilterValue) -> Option<&str> {
    match fv {
        FilterValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Expression NULL results are skipped by all aggregates, including COUNT.
/// Empty SUM/COUNT return 0; empty AVG/MIN/MAX return NULL. Literal NULL
/// SUM is finalized as NULL by build_aggregate_object. Legacy field
/// behaviour is unchanged: COUNT(field) counts present NULLs, and MIN/MAX
/// can retain a NULL extreme. Expression SUM/AVG require numeric values;
/// MIN/MAX require comparable values (including comparable numeric families).
pub(super) fn step_aggregate_input(
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
    let key = interner
        .touch_ind("agg_input")
        .map_err(|error| DbError::Validation(error.into()))?
        .into_key();
    acc.field_path = Some(vec![key.clone()]);
    acc.all_field = false;
    let mut record = new_map_wc(1);
    record.insert(key, leaf);
    acc.step(&InnerValue::Map(record));
    Ok(())
}
