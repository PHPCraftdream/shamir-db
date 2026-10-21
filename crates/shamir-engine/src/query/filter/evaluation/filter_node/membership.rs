//! Set-membership and FTS-count helpers for the compiled filter tree.

use std::cmp::Ordering;

use shamir_collections::TSet;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::value::QueryValue;

use indexmap::Equivalent;
use std::hash::{Hash, Hasher};

use crate::query::filter::evaluation::semantics::numeric_cmp::cmp_i64_f64;

pub(super) fn set_contains_coercing(set: &TSet<QueryValue>, sr: ScalarRef<'_>) -> bool {
    match sr {
        ScalarRef::Int(n) => {
            // Same-type probe.
            if set.contains(&QueryValue::Int(n)) {
                return true;
            }
            // Cross-type F64 probe — F-2 (#791): `n as f64` is only a
            // valid probe key when it round-trips back to `n` exactly
            // (`cmp_i64_f64(n, n as f64) == Equal`). For `|n| >= 2^53`,
            // `n as f64` can round to a DIFFERENT f64 than what the set
            // actually stores — probing that rounded value would be a
            // false miss (the real equal f64, if any, differs from the
            // rounded one) or, if the set happens to also contain that
            // unrelated rounded f64 under a different filter entry, a
            // false hit. Skip the probe entirely once exactness can't be
            // guaranteed; `TSet<QueryValue>` cannot be range-probed for
            // "any f64 equal to n" without a linear scan, and no in-set
            // f64 can equal an out-of-round-trip-range n anyway.
            let f = n as f64;
            if cmp_i64_f64(n, f) == Some(Ordering::Equal) {
                set.contains(&QueryValue::F64(f))
            } else {
                false
            }
        }
        ScalarRef::F64(f) => {
            // Same-type probe.
            if set.contains(&QueryValue::F64(f)) {
                return true;
            }
            // Cross-type Int probe — F-2 (#791): exact via the shared
            // `numeric_cmp::cmp_i64_f64` instead of the old lossy bounds
            // clamp (`f >= i64::MIN as f64 && f <= i64::MAX as f64`), which
            // had the same off-by-one risk W-1/CR-D3 fixed elsewhere:
            // `i64::MAX as f64` rounds UP to `2^63` (since `i64::MAX ==
            // 2^63 - 1` is not exactly representable as `f64`), so `f ==
            // 2^63.0` incorrectly passed the old `<=` check. This function
            // only needs an equality test, not full ordering, so probe
            // every candidate `i64` whose `cmp_i64_f64` result is `Equal`.
            if f.is_finite() {
                if let Some(n) = exact_i64_equal_to_f64(f) {
                    return set.contains(&QueryValue::Int(n));
                }
            }
            false
        }
        ScalarRef::Null => set.contains(&QueryValue::Null),
        ScalarRef::Bool(b) => set.contains(&QueryValue::Bool(b)),
        // F-21 group: probe via a borrowed key (`StrProbe`/`BinProbe`)
        // instead of allocating an owned `QueryValue::Str(String)` /
        // `QueryValue::Bin(Vec<u8>)` per row just to hash+compare it once —
        // this was the one allocating arm left in an otherwise zero-copy
        // `scalar_at`-based probe.
        ScalarRef::Str(s) => set.contains(&StrProbe(s)),
        ScalarRef::Bin(b) => set.contains(&BinProbe(b)),
    }
}

/// Zero-alloc probe key for `TSet<QueryValue>::contains` — hashes and
/// compares identically to `QueryValue::Str(String)` without ever
/// allocating an owned `String`/`QueryValue` per probe.
///
/// Correctness of the hash match: `Value::hash`'s `Str` arm hashes the
/// variant's discriminant followed by the `String`'s content; `String`'s
/// `Hash` impl delegates byte-for-byte to `str::hash`, so hashing the SAME
/// discriminant + the same bytes via a borrowed `&str` here (instead of an
/// owned `String`) produces an identical digest under the set's `FxHasher`.
struct StrProbe<'a>(&'a str);

impl Hash for StrProbe<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        str_discriminant().hash(state);
        self.0.hash(state);
    }
}

impl Equivalent<QueryValue> for StrProbe<'_> {
    fn equivalent(&self, key: &QueryValue) -> bool {
        matches!(key, QueryValue::Str(s) if s.as_str() == self.0)
    }
}

/// Zero-alloc probe key for `TSet<QueryValue>::contains` — the `Bin`
/// sibling of [`StrProbe`]. `Vec<u8>`'s `Hash` impl delegates to `[u8]`'s,
/// so hashing the same bytes via a borrowed `&[u8]` matches exactly.
struct BinProbe<'a>(&'a [u8]);

impl Hash for BinProbe<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        bin_discriminant().hash(state);
        self.0.hash(state);
    }
}

impl Equivalent<QueryValue> for BinProbe<'_> {
    fn equivalent(&self, key: &QueryValue) -> bool {
        matches!(key, QueryValue::Bin(b) if b.as_slice() == self.0)
    }
}

/// `QueryValue::Str`'s enum discriminant. Building `QueryValue::Str(String::new())`
/// is heap-alloc-free (`String::new()` never allocates) — this is a stack-only
/// tag extraction, not a per-probe allocation.
#[inline]
fn str_discriminant() -> std::mem::Discriminant<QueryValue> {
    std::mem::discriminant(&QueryValue::Str(String::new()))
}

/// `QueryValue::Bin`'s enum discriminant — see [`str_discriminant`].
/// `Vec::new()` never allocates.
#[inline]
fn bin_discriminant() -> std::mem::Discriminant<QueryValue> {
    std::mem::discriminant(&QueryValue::Bin(Vec::new()))
}

/// If `f` has an exact `i64` equivalent (`cmp_i64_f64(n, f) ==
/// Some(Equal)`), return that `n`. Otherwise `None` — either `f` has a
/// nonzero fractional part or is out of `i64`'s range.
///
/// `f.floor() as i64` is the only candidate that could possibly be equal
/// (any other integer differs from `f` by at least 1), so this is a single
/// `cmp_i64_f64` probe, not a search.
#[inline]
fn exact_i64_equal_to_f64(f: f64) -> Option<i64> {
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if !(I64_MIN_AS_F64..I64_MAX_EXCLUSIVE_UPPER_BOUND).contains(&f) {
        return None;
    }
    let candidate = f.floor() as i64;
    if cmp_i64_f64(candidate, f) == Some(std::cmp::Ordering::Equal) {
        Some(candidate)
    } else {
        None
    }
}

/// Probe a `TSet<QueryValue>` for membership using the SAME coercion rules
/// as `scalar_ref_cmp_qv` (which the old per-row linear scan used) — the
/// `QueryValue`-native sibling of [`set_contains_coercing`].
///
/// This closes the gap acknowledged in the `InSet` comment (~line 448):
/// the all-literal fast-path nodes (`InSet`, `ContainsAnySet`,
/// `ContainsAllSet`) previously used exact `TSet::contains`/`swap_remove`,
/// so `{"$in": [1.0]}` against an `Int(1)` field did NOT match — while the
/// dynamic branch (`FilterNode::In`) DID match via `set_contains_coercing`
/// /`scalar_ref_cmp_qv`. A filter's answer must not depend on whether its
/// value list happens to be fully literal.
///
/// Coercion rules (identical to [`set_contains_coercing`]):
/// - `Int(n)`  → probe `Int(n)` AND, if `n as f64` round-trips exactly back
///   to `n`, `F64(n as f64)`.
/// - `F64(f)`  → probe `F64(f)` AND, if `f` has an exact `i64` equivalent,
///   `Int(that i64)`.
/// - Other types → single exact probe (no cross-type coercion).
///
/// F-2 (#791): both cross-type probes now go through the shared exact
/// comparator (`numeric_cmp::cmp_i64_f64` via [`exact_i64_equal_to_f64`] /
/// the round-trip check below) instead of the old lossy `as f64`/`as i64`
/// casts + a bounds clamp that had the same off-by-one risk W-1/CR-D3 fixed
/// elsewhere (`i64::MAX as f64` rounds up to the exact power of two `2^63`,
/// so a naive `f <= i64::MAX as f64` clamp wrongly passes `f == 2^63.0`).
#[inline]
pub(super) fn set_contains_coercing_qv(set: &TSet<QueryValue>, qv: &QueryValue) -> bool {
    match qv {
        QueryValue::Int(n) => {
            // Same-type probe.
            if set.contains(qv) {
                return true;
            }
            // Cross-type F64 probe — only when `*n as f64` round-trips
            // exactly back to `n` (see `set_contains_coercing`'s `Int` arm
            // for the full derivation of why this guard is required).
            let f = *n as f64;
            if cmp_i64_f64(*n, f) == Some(Ordering::Equal) {
                set.contains(&QueryValue::F64(f))
            } else {
                false
            }
        }
        QueryValue::F64(f) => {
            // Same-type probe.
            if set.contains(qv) {
                return true;
            }
            // Cross-type Int probe — exact via `exact_i64_equal_to_f64`.
            if f.is_finite() {
                if let Some(n) = exact_i64_equal_to_f64(*f) {
                    return set.contains(&QueryValue::Int(n));
                }
            }
            false
        }
        // All other types: exact match (no coercion in scalar_ref_cmp_qv).
        _ => set.contains(qv),
    }
}

/// Coercing `get_index_of` for `ContainsAllSet`'s bitmask scan.
///
/// Mirrors [`set_contains_coercing_qv`] but returns the INDEX of whichever
/// coercion-equivalent representation is present (`values` is a `TSet`, i.e.
/// an `IndexSet` — `get_index_of` is an O(1) lookup, no clone/removal of the
/// set itself). This replaces the old `swap_remove_coercing_qv`, which
/// mutated a per-record `values.clone()` — cloning every required
/// `QueryValue` (including any owned `String`/`Vec<u8>` payload) on EVERY
/// row just to track which ones had been found so far.
#[inline]
fn index_of_coercing_qv(set: &TSet<QueryValue>, qv: &QueryValue) -> Option<usize> {
    match qv {
        QueryValue::Int(n) => {
            if let Some(i) = set.get_index_of(qv) {
                return Some(i);
            }
            let f = *n as f64;
            if cmp_i64_f64(*n, f) == Some(Ordering::Equal) {
                set.get_index_of(&QueryValue::F64(f))
            } else {
                None
            }
        }
        QueryValue::F64(f) => {
            if let Some(i) = set.get_index_of(qv) {
                return Some(i);
            }
            if f.is_finite() {
                if let Some(n) = exact_i64_equal_to_f64(*f) {
                    return set.get_index_of(&QueryValue::Int(n));
                }
            }
            None
        }
        _ => set.get_index_of(qv),
    }
}

/// `$contains_all` membership scan for `ContainsAllSet` — dispatches on the
/// field's container kind, then tracks "found" required values via a
/// bitmask keyed by INDEX into `values` (see [`contains_all_scan`]) instead
/// of cloning the whole required-values set per record.
#[inline]
pub(super) fn contains_all_set_match(field_qv: &QueryValue, values: &TSet<QueryValue>) -> bool {
    match field_qv {
        QueryValue::List(list) => contains_all_scan(list.iter(), values),
        QueryValue::Set(set) => contains_all_scan(set.iter(), values),
        _ => false,
    }
}

/// Single pass over `field_items`, marking each required value in `values`
/// found via a bitmask instead of `swap_remove`-ing it out of a per-record
/// clone. `values.len() <= 64` (the overwhelming common case) uses a `u64`
/// bitmask — zero heap allocation, mirroring `FilterNode::FtsMatch`'s
/// AND-mode bitmask a few arms up. Larger sets fall back to a `Vec<bool>`,
/// still far cheaper than cloning N owned `QueryValue`s every row.
///
/// An empty `values` set is vacuously satisfied (mirrors the old
/// `remaining.is_empty()` check starting true when `values` was empty).
fn contains_all_scan<'a>(
    field_items: impl Iterator<Item = &'a QueryValue>,
    values: &TSet<QueryValue>,
) -> bool {
    let n = values.len();
    if n == 0 {
        return true;
    }
    if n <= 64 {
        let target: u64 = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        let mut seen: u64 = 0;
        for item in field_items {
            if let Some(idx) = index_of_coercing_qv(values, item) {
                seen |= 1u64 << idx;
                if seen == target {
                    return true;
                }
            }
        }
        seen == target
    } else {
        let mut seen = vec![false; n];
        let mut remaining = n;
        for item in field_items {
            if let Some(idx) = index_of_coercing_qv(values, item) {
                if !seen[idx] {
                    seen[idx] = true;
                    remaining -= 1;
                    if remaining == 0 {
                        return true;
                    }
                }
            }
        }
        remaining == 0
    }
}
