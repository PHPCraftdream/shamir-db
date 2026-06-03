//! Canonical cross-type total order for [`InnerValue`].
//!
//! [`compare`] defines a **total** order (reflexive, transitive,
//! antisymmetric, and *total* -- every pair is comparable) so that
//! `min`, `max`, `sort`, `median`, `mode`, and `percentile` work across
//! mixed-type value streams without panic or "undefined" results.
//!
//! ## Type ranks
//!
//! | Rank | Variant(s)           |
//! |------|----------------------|
//! | 0    | Null                 |
//! | 1    | Bool                 |
//! | 2    | Int, F64, Dec, Big   |
//! | 3    | Str                  |
//! | 4    | Bin                  |
//! | 5    | List                 |
//! | 6    | Set                  |
//! | 7    | Map                  |
//!
//! Values of different ranks compare by rank alone.
//!
//! ## Within-type ordering
//!
//! - **Numeric (rank 2):** All four numeric variants share rank 2 and
//!   compare by *numeric value* across subtypes. `Int 5`, `Dec 5.0`,
//!   and `F64 5.0` are `Equal`. Exact subtypes (Int, Dec) are compared
//!   via [`Decimal`]; mixed comparisons involving `F64` or `Big` fall
//!   back to `f64`. `NaN` sorts *last* among numerics, and
//!   `NaN == NaN` (total-order requirement).
//! - **Bool:** `false < true`.
//! - **Str:** lexicographic via [`str::cmp`].
//! - **Bin:** byte-lexicographic via slice `cmp`.
//! - **List:** element-wise via recursive [`compare`]; on equal prefix
//!   the shorter list is `Less`.
//! - **Set / Map:** coarse order by `.len()`; equal length yields
//!   `Equal`. This is intentionally loose and may be refined later.

use num_bigint::BigInt;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use shamir_types::types::value::InnerValue;
use std::cmp::Ordering;

/// Canonical total order over [`InnerValue`].
///
/// Never panics, never returns an "undefined" result. See module docs
/// for the full rank table and within-type semantics.
pub fn compare(a: &InnerValue, b: &InnerValue) -> Ordering {
    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }
    // Same rank -- dispatch to within-type comparison.
    match (a, b) {
        (InnerValue::Null, InnerValue::Null) => Ordering::Equal,
        (InnerValue::Bool(x), InnerValue::Bool(y)) => x.cmp(y),
        // Numeric rank (Int | F64 | Dec | Big) -- compare by value.
        (na, nb) if ra == 2 => compare_numeric(na, nb),
        (InnerValue::Str(x), InnerValue::Str(y)) => x.cmp(y),
        (InnerValue::Bin(x), InnerValue::Bin(y)) => x.cmp(y),
        (InnerValue::List(x), InnerValue::List(y)) => compare_lists(x, y),
        (InnerValue::Set(x), InnerValue::Set(y)) => x.len().cmp(&y.len()),
        (InnerValue::Map(x), InnerValue::Map(y)) => x.len().cmp(&y.len()),
        // Unreachable: same rank means same variant family.
        _ => Ordering::Equal,
    }
}

/// Assign the type rank used for cross-type ordering.
fn type_rank(v: &InnerValue) -> u8 {
    match v {
        InnerValue::Null => 0,
        InnerValue::Bool(_) => 1,
        InnerValue::Int(_) | InnerValue::F64(_) | InnerValue::Dec(_) | InnerValue::Big(_) => 2,
        InnerValue::Str(_) => 3,
        InnerValue::Bin(_) => 4,
        InnerValue::List(_) => 5,
        InnerValue::Set(_) => 6,
        InnerValue::Map(_) => 7,
    }
}

// ---------------------------------------------------------------------------
// Numeric cross-subtype comparison
// ---------------------------------------------------------------------------

/// Compare two numeric `InnerValue`s by *value* across subtypes.
///
/// Strategy:
/// - Int vs Int: direct i64 comparison.
/// - Dec vs Dec: direct Decimal comparison.
/// - Int vs Dec / Dec vs Int: promote Int to Decimal.
/// - Anything involving F64: convert both sides to f64.
/// - Anything involving Big: try Decimal first (if Big fits); else f64.
/// - NaN sorts last; NaN == NaN for totality.
fn compare_numeric(a: &InnerValue, b: &InnerValue) -> Ordering {
    // Fast paths for same-subtype.
    match (a, b) {
        (InnerValue::Int(x), InnerValue::Int(y)) => return x.cmp(y),
        (InnerValue::Dec(x), InnerValue::Dec(y)) => return x.cmp(y),
        (InnerValue::F64(x), InnerValue::F64(y)) => return cmp_f64(*x, *y),
        (InnerValue::Big(x), InnerValue::Big(y)) => return x.cmp(y),
        _ => {}
    }

    // Cross-subtype: exact (Decimal) path for Int/Dec pairs.
    match (a, b) {
        (InnerValue::Int(x), InnerValue::Dec(d)) => return Decimal::from(*x).cmp(d),
        (InnerValue::Dec(d), InnerValue::Int(y)) => return d.cmp(&Decimal::from(*y)),
        _ => {}
    }

    // Anything else (F64 or Big involved) -- convert to f64.
    let fa = to_f64(a);
    let fb = to_f64(b);
    cmp_f64(fa, fb)
}

/// Total-order f64 comparison: NaN sorts last, NaN == NaN.
fn cmp_f64(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater, // NaN last
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// Best-effort conversion to f64 for cross-subtype numeric comparison.
fn to_f64(v: &InnerValue) -> f64 {
    match v {
        InnerValue::Int(n) => *n as f64,
        InnerValue::F64(f) => *f,
        InnerValue::Dec(d) => d.to_f64().unwrap_or(f64::NAN),
        InnerValue::Big(b) => big_to_f64(b),
        _ => f64::NAN,
    }
}

/// Convert a BigInt to f64 (lossy but deterministic).
fn big_to_f64(b: &BigInt) -> f64 {
    b.to_f64().unwrap_or_else(|| {
        // Extremely large BigInt that overflows f64 → use sign.
        if b.sign() == num_bigint::Sign::Minus {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }
    })
}

/// Element-wise list comparison with recursive [`compare`].
fn compare_lists(a: &[InnerValue], b: &[InnerValue]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        let c = compare(x, y);
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests;
