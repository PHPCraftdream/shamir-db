//! Canonical cross-type total order for [`QueryValue`].
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
//!   and F64 5.0 are equal. All comparisons are exact, including the
//!   binary rational represented by a float. NaN sorts last and all NaNs
//!   compare equal; signed zeros compare equal.
//! - **Bool:** `false < true`.
//! - **Str:** lexicographic via [`str::cmp`].
//! - **Bin:** byte-lexicographic via slice `cmp`.
//! - **List:** element-wise via recursive [`compare`]; on equal prefix
//!   the shorter list is `Less`.
//! - **Set / Map:** canonicalized to a sorted form (Set elements sorted by
//!   [`compare`], Map entries sorted by key), then compared element-wise with
//!   the same tail-length rule as List. Two structurally-equal containers
//!   built in different insertion order still compare `Equal`.

use crate::functions::numeric_exact::ExactNumber;
use shamir_types::types::common::{TMap, TSet};
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;

/// Canonical total order over [`QueryValue`].
///
/// Never panics, never returns an "undefined" result. See module docs
/// for the full rank table and within-type semantics.
pub fn compare(a: &QueryValue, b: &QueryValue) -> Ordering {
    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }
    // Same rank -- dispatch to within-type comparison.
    match (a, b) {
        (QueryValue::Null, QueryValue::Null) => Ordering::Equal,
        (QueryValue::Bool(x), QueryValue::Bool(y)) => x.cmp(y),
        // Numeric rank (Int | F64 | Dec | Big) -- compare by value.
        (na, nb) if ra == 2 => compare_numeric(na, nb),
        (QueryValue::Str(x), QueryValue::Str(y)) => x.cmp(y),
        (QueryValue::Bin(x), QueryValue::Bin(y)) => x.cmp(y),
        (QueryValue::List(x), QueryValue::List(y)) => compare_lists(x, y),
        (QueryValue::Set(x), QueryValue::Set(y)) => compare_sets(x, y),
        (QueryValue::Map(x), QueryValue::Map(y)) => compare_maps(x, y),
        // Unreachable: same rank means same variant family.
        _ => Ordering::Equal,
    }
}

/// Assign the type rank used for cross-type ordering.
fn type_rank(v: &QueryValue) -> u8 {
    match v {
        QueryValue::Null => 0,
        QueryValue::Bool(_) => 1,
        QueryValue::Int(_) | QueryValue::F64(_) | QueryValue::Dec(_) | QueryValue::Big(_) => 2,
        QueryValue::Str(_) => 3,
        QueryValue::Bin(_) => 4,
        QueryValue::List(_) => 5,
        QueryValue::Set(_) => 6,
        QueryValue::Map(_) => 7,
    }
}

// ---------------------------------------------------------------------------
// Numeric cross-subtype comparison
// ---------------------------------------------------------------------------

/// Compare exact numeric values, including infinities and NaNs.
fn compare_numeric(a: &QueryValue, b: &QueryValue) -> Ordering {
    match (a, b) {
        (QueryValue::Int(x), QueryValue::Int(y)) => x.cmp(y),
        (QueryValue::Dec(x), QueryValue::Dec(y)) => x.cmp(y),
        (QueryValue::Big(x), QueryValue::Big(y)) => x.cmp(y),
        _ => ExactNumber::from_value(a).compare(&ExactNumber::from_value(b)),
    }
}

/// Element-wise list comparison with recursive [`compare`].
fn compare_lists(a: &[QueryValue], b: &[QueryValue]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        let c = compare(x, y);
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

/// Canonicalized set comparison.
///
/// `TSet<QueryValue>` is insertion-ordered (NOT sorted), so two
/// structurally-equal sets built in different insertion order must still
/// compare `Equal`. We canonicalize both sides by sorting elements using
/// [`compare`] itself as the sort comparator (recursive — `QueryValue`
/// nesting is bounded in practice), then compare element-wise with the same
/// tail-length rule as [`compare_lists`].
///
/// O(n log n) per comparison — acceptable since `compare` on Set/Map is a
/// cold path (aggregates over container columns are not a hot path).
fn compare_sets(a: &TSet<QueryValue>, b: &TSet<QueryValue>) -> Ordering {
    let mut av: Vec<&QueryValue> = a.iter().collect();
    let mut bv: Vec<&QueryValue> = b.iter().collect();
    av.sort_by(|x, y| compare(x, y));
    bv.sort_by(|x, y| compare(x, y));
    for (x, y) in av.iter().zip(bv.iter()) {
        let c = compare(x, y);
        if c != Ordering::Equal {
            return c;
        }
    }
    av.len().cmp(&bv.len())
}

/// Canonicalized map comparison.
///
/// `TMap<String, QueryValue>` is insertion-ordered (NOT sorted). We
/// canonicalize by sorting entries by key (`String` has a natural `Ord`),
/// then compare element-wise: first by key, then — on equal keys — by
/// recursively calling [`compare`] on the values. Unequal lengths after
/// the common prefix: shorter is `Less`.
fn compare_maps(a: &TMap<String, QueryValue>, b: &TMap<String, QueryValue>) -> Ordering {
    let mut av: Vec<(&String, &QueryValue)> = a.iter().collect();
    let mut bv: Vec<(&String, &QueryValue)> = b.iter().collect();
    av.sort_by(|x, y| x.0.cmp(y.0));
    bv.sort_by(|x, y| x.0.cmp(y.0));
    for (x, y) in av.iter().zip(bv.iter()) {
        let kc = x.0.cmp(y.0);
        if kc != Ordering::Equal {
            return kc;
        }
        let vc = compare(x.1, y.1);
        if vc != Ordering::Equal {
            return vc;
        }
    }
    av.len().cmp(&bv.len())
}
