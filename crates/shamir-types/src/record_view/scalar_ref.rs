//! Borrowed scalar leaf — the cross-impl currency for the [`RecordRef`] trait.
//!
//! Mirrors [`eval_bytes::RawScalar`] in shape (Null/Bool/Int/F64/Str/Bin) but
//! is a public, `PartialEq + Debug` type suitable for assertions and Stage-3
//! consumer code. Only scalar types that [`compare_values`] and
//! `eval_bytes::compare_raw_to_filter` handle are represented — containers
//! (Map/Array), `Dec`, `Big`, `Set` are NOT scalars and resolve to `None` from
//! [`RecordRef::scalar_at`].
//!
//! [`RecordRef`]: super::RecordRef
//! [`compare_values`]: (engine-internal — see `resolve.rs`)

use std::cmp::Ordering;

use crate::types::value::{InnerValue, QueryValue};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// A borrowed scalar leaf extracted from a record. The cross-impl currency
/// that makes `InnerValue` and `RecordView` interchangeable at the trait level.
///
/// # Why these variants and no others
///
/// * **Null, Bool, Int, F64, Str, Bin** — these are the types that
///   `compare_values` (tree path) and `compare_raw_to_filter` (bytes path)
///   can compare. They are the scalar types that filter-eval and index-extract
///   (the first Stage-3 consumers) need.
/// * **Dec / Big** — NOT directly extractable as a `ScalarRef`; `scalar_at`
///   returns `None` for these (the container fallback `materialize_at` is the
///   lens path for them). The comparison helpers (`scalar_ref_cmp` /
///   `scalar_ref_cmp_qv`) DO bridge `Int`/`F64` record fields against `Dec`/
///   `Big` filter operands cross-type (exact for `Int`↔`Dec`, f64 fallback
///   otherwise) so a `$fn` returning `Dec` matches numerically.
/// * **List / Set / Map** — containers, not scalars. Mapped to `None`.
///
/// # NaN caveat
///
/// `PartialEq` for `F64` compares with `==`, so `NaN != NaN`. The parity
/// tests use a helper that compares `f64` via `to_bits()` to handle this.
#[derive(Debug, Clone, Copy)]
pub enum ScalarRef<'a> {
    /// Null / nil.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed 64-bit integer (all int widths collapse here, matching `InnerValue::Int`).
    Int(i64),
    /// 64-bit float (F32 widened, matching `InnerValue::F64`).
    F64(f64),
    /// Borrowed string slice.
    Str(&'a str),
    /// Borrowed binary slice.
    Bin(&'a [u8]),
}

impl<'a> PartialEq for ScalarRef<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ScalarRef::Null, ScalarRef::Null) => true,
            (ScalarRef::Bool(a), ScalarRef::Bool(b)) => a == b,
            (ScalarRef::Int(a), ScalarRef::Int(b)) => a == b,
            (ScalarRef::F64(a), ScalarRef::F64(b)) => a.to_bits() == b.to_bits(),
            (ScalarRef::Str(a), ScalarRef::Str(b)) => a == b,
            (ScalarRef::Bin(a), ScalarRef::Bin(b)) => a == b,
            _ => false,
        }
    }
}

impl<'a> Eq for ScalarRef<'a> {}

/// Exact `i64` vs `f64` comparison — F-2 (#791) follow-up.
///
/// `scalar_ref_cmp`/`scalar_ref_cmp_qv` are the HOT comparison path for
/// `FilterNode::Compare` (every ordinary `Eq`/`Gt`/`Gte`/`Lt`/`Lte` WHERE
/// clause whose field resolves to a borrowed scalar) — more heavily hit
/// than any of the fast-path sites consolidated in
/// `shamir-engine::query::filter::numeric_cmp`. This crate sits BELOW
/// `shamir-engine` in the dependency graph, so it cannot reuse that
/// module; this is the same exact bounds-check + floor/fract technique
/// (CR-D3, #784), reproduced here because the two crates cannot share a
/// single copy without introducing a new shared crate — out of scope for
/// F-2, which is an engine-internal consolidation. See
/// `shamir-engine::query::filter::numeric_cmp::cmp_i64_f64`'s doc comment
/// for the full derivation of why this technique is exact.
#[inline]
fn cmp_i64_f64(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if f < I64_MIN_AS_F64 {
        return Some(Ordering::Greater);
    }
    if f >= I64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(Ordering::Less);
    }
    let f_floor = f.floor();
    let f_floor_i64 = f_floor as i64;
    match i.cmp(&f_floor_i64) {
        Ordering::Equal => {
            if f > f_floor {
                Some(Ordering::Less)
            } else {
                Some(Ordering::Equal)
            }
        }
        other => Some(other),
    }
}

/// Lossy `Decimal` → `f64` (NaN on overflow). Used for cross-type comparison
/// where one side is a float — mirrors the accepted f64-precision tradeoff of
/// the existing `Int`↔`F64` arms.
#[inline]
fn dec_to_f64(d: &Decimal) -> f64 {
    d.to_f64().unwrap_or(f64::NAN)
}

/// Lossy `BigInt` → `f64` (NaN on overflow). Precision loss for large values
/// is an accepted, separately-tracked tradeoff; this only stops `Big` from
/// being a silent `None`/no-match.
#[inline]
fn big_to_f64(b: &num_bigint::BigInt) -> f64 {
    b.to_f64().unwrap_or(f64::NAN)
}

/// Compare a borrowed [`ScalarRef`] against an [`InnerValue`] scalar literal.
///
/// Mirrors `compare_values` (engine `resolve.rs`) arm-for-arm — Null==Null,
/// Bool, Int/Int, **cross-type Int/F64**, F64/F64, Str/Str, plus the
/// `Dec`/`Big` cross-type arms (record-field `Int`/`F64` vs filter-literal
/// `Dec`/`Big`). Returns `None` for non-comparable pairs (mismatched type
/// families that have no numeric bridge, containers, `Bin`).
///
/// This is the reusable comparison helper that Stage-3 consumers call after
/// extracting a `ScalarRef` via [`RecordRef::scalar_at`], replacing the old
/// `resolve_field` + `compare_values` pattern one call-site at a time.
///
/// [`RecordRef::scalar_at`]: super::RecordRef::scalar_at
#[inline]
pub fn scalar_ref_cmp(a: ScalarRef<'_>, b: &InnerValue) -> Option<Ordering> {
    match (a, b) {
        (ScalarRef::Null, InnerValue::Null) => Some(Ordering::Equal),
        (ScalarRef::Bool(a), InnerValue::Bool(b)) => Some(a.cmp(b)),
        (ScalarRef::Int(a), InnerValue::Int(b)) => Some(a.cmp(b)),
        // Int<->F64: F-2 (#791) follow-up — exact via `cmp_i64_f64` (see its
        // doc comment), replacing the lossy `a as f64`/`*b as f64` casts.
        (ScalarRef::Int(a), InnerValue::F64(b)) => cmp_i64_f64(a, *b),
        (ScalarRef::F64(a), InnerValue::Int(b)) => cmp_i64_f64(*b, a).map(Ordering::reverse),
        (ScalarRef::F64(a), InnerValue::F64(b)) => a.partial_cmp(b),
        (ScalarRef::Str(a), InnerValue::Str(b)) => Some(a.cmp(b.as_str())),
        // Dec cross-type: record field (Int/F64) vs literal Dec. Int↔Dec is
        // exact (`Decimal` represents every `i64`); F64↔Dec uses the f64
        // fallback (mirrors the Int↔F64 tradeoff).
        (ScalarRef::Int(a), InnerValue::Dec(b)) => Some(Decimal::from(a).cmp(b)),
        (ScalarRef::F64(a), InnerValue::Dec(b)) => a.partial_cmp(&dec_to_f64(b)),
        // Big cross-type: f64 fallback (precision loss accepted — see `big_to_f64`).
        (ScalarRef::Int(a), InnerValue::Big(b)) => (a as f64).partial_cmp(&big_to_f64(b)),
        (ScalarRef::F64(a), InnerValue::Big(b)) => a.partial_cmp(&big_to_f64(b)),
        _ => None,
    }
}

/// Compare a borrowed [`ScalarRef`] against a [`QueryValue`] scalar literal.
///
/// C6 (#80): the name-keyed twin of [`scalar_ref_cmp`]. `Value<String>` and
/// `Value<InternerKey>` order identically for every scalar arm (the key type
/// only matters for `Map`, which is not a scalar and resolves to `None`
/// here), so this is byte-identical to the InnerValue form for scalars.
#[inline]
pub fn scalar_ref_cmp_qv(a: ScalarRef<'_>, b: &QueryValue) -> Option<Ordering> {
    match (a, b) {
        (ScalarRef::Null, QueryValue::Null) => Some(Ordering::Equal),
        (ScalarRef::Bool(a), QueryValue::Bool(b)) => Some(a.cmp(b)),
        (ScalarRef::Int(a), QueryValue::Int(b)) => Some(a.cmp(b)),
        // Int<->F64: F-2 (#791) follow-up — exact via `cmp_i64_f64` (see its
        // doc comment), replacing the lossy `a as f64`/`*b as f64` casts.
        (ScalarRef::Int(a), QueryValue::F64(b)) => cmp_i64_f64(a, *b),
        (ScalarRef::F64(a), QueryValue::Int(b)) => cmp_i64_f64(*b, a).map(Ordering::reverse),
        (ScalarRef::F64(a), QueryValue::F64(b)) => a.partial_cmp(b),
        (ScalarRef::Str(a), QueryValue::Str(b)) => Some(a.cmp(b.as_str())),
        // Dec cross-type (see `scalar_ref_cmp` for rationale).
        (ScalarRef::Int(a), QueryValue::Dec(b)) => Some(Decimal::from(a).cmp(b)),
        (ScalarRef::F64(a), QueryValue::Dec(b)) => a.partial_cmp(&dec_to_f64(b)),
        // Big cross-type (see `scalar_ref_cmp` for rationale).
        (ScalarRef::Int(a), QueryValue::Big(b)) => (a as f64).partial_cmp(&big_to_f64(b)),
        (ScalarRef::F64(a), QueryValue::Big(b)) => a.partial_cmp(&big_to_f64(b)),
        _ => None,
    }
}
