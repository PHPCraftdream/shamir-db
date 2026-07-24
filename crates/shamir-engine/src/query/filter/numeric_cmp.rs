//! Shared exact numeric cross-type comparators — CR-D3 (#784), F-2 (#791).
//!
//! `cmp_i64_f64` and `cmp_u64_f64` are the single source of truth for
//! `Int`/`U64` vs `F64` comparison across every fast path in this crate:
//! the general evaluator (`resolve::compare_values`), ORDER BY
//! (`read::order::QvSortKey`), the bytes-level pre-filter
//! (`eval_bytes::compare_raw_to_filter`), `IN`/`NOT IN` set membership
//! (`filter_node::set_contains_coercing`), and MIN/MAX aggregation
//! (`read::aggregate::OwnedScalar::cmp_scalar`).
//!
//! Before this consolidation, `cmp_i64_f64` was fixed (CR-D3, #784) in
//! exactly TWO places — `resolve.rs` and `order.rs` — as a byte-for-byte
//! duplicated private fn, while three OTHER numeric-comparison sites in the
//! same crate kept the old lossy `as f64`/`as i64` cast + `partial_cmp`
//! technique. A filter's answer must not depend on which fast path
//! evaluated it; this module makes that an invariant instead of a
//! per-site convention.

use std::cmp::Ordering;

/// Exact `i64` vs `f64` comparison — CR-D3 (#784).
///
/// `f64` has an 11-bit exponent, enough to represent every integer up to
/// `2^63` in MAGNITUDE (though not every value at high magnitude, since the
/// 52-bit mantissa runs out of precision past `2^53`) — this is exactly what
/// makes a bounds-check + `floor`/`fract` technique exact without
/// arbitrary-precision arithmetic (no `BigInt` needed, unlike `Big`↔`F64`
/// which is an inherent, unfixable approximation because `F64` itself is the
/// imprecise side there).
///
/// `i64::MIN == -2^63` and `i64::MAX == 2^63 - 1` — both `-2^63` and `2^63`
/// are exact powers of two, always exactly representable as `f64` literals.
/// `f < -2^63` means `f < i64::MIN <= i`, so `i > f`. `f >= 2^63` means
/// `f >= i64::MAX + 1 > i64::MAX >= i`, so `i < f`. For finite `f` in
/// `[-2^63, 2^63)`: any `f64` with `|f| >= 2^53` has no fractional bits
/// available at all (the entire 52-bit mantissa is consumed by the integer
/// part at that exponent), so `f.fract() == 0.0` identically and
/// `f.floor() == f` exactly for that whole magnitude range; below `2^53`,
/// `floor`/`fract` behave as normal exact-integer-valued doubles. Either
/// way, `f.floor()` is an exact integer value within `[-2^63, 2^63 - 1]`,
/// i.e. `i64`'s full range, so `f.floor() as i64` is a lossless cast. From
/// there, comparing `i` against `f_floor_i64` as plain integers settles
/// everything except the exact-equal case, where comparing `f` against
/// `f_floor` directly breaks the tie: `i == f.floor()` and `f > f_floor`
/// means `f > i`. This must compare against `f_floor`, NOT `f.fract()` --
/// `f.fract()` is `f - f.trunc()` (truncation-based, sign-preserving), so
/// for negative `f` it is negative or zero, never positive, even when `f`
/// has a nonzero fractional part (e.g. `(-0.5_f64).fract() == -0.5`). Only
/// `f - f.floor()` is guaranteed `>= 0` for every finite `f`, positive or
/// negative alike.
#[inline]
pub(crate) fn cmp_i64_f64(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None; // preserve the EXISTING NaN convention this codebase
                     // already uses for F64<->F64 (partial_cmp's own NaN
                     // handling) -- do not invent new NaN semantics here.
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        }); // any finite i64 is < +inf, > -inf.
    }
    // f is finite from here on. Bound f against i64's range using EXACT
    // powers of two.
    const I64_MIN_AS_F64: f64 = -9223372036854775808.0; // -2^63, exact
    const I64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 9223372036854775808.0; // 2^63, exact
    if f < I64_MIN_AS_F64 {
        return Some(Ordering::Greater); // i (>= i64::MIN) > f
    }
    if f >= I64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(Ordering::Less); // i (<= i64::MAX) < f
    }
    // f is finite and within [-2^63, 2^63) -- f.floor() is an exact integer
    // value in that range, losslessly representable as i64 (see derivation
    // above).
    let f_floor = f.floor();
    let f_floor_i64 = f_floor as i64;
    match i.cmp(&f_floor_i64) {
        Ordering::Equal => {
            // i == floor(f) exactly. f >= f_floor always (floor rounds
            // DOWN, never up) -- f > f_floor iff f has ANY nonzero
            // fractional part, positive or negative f alike. Comparing
            // against f_floor directly (not f.fract(), which is
            // TRUNC-based and sign-preserving -- negative for negative
            // fractional f, the bug this replaces) is correct for every
            // sign.
            if f > f_floor {
                Some(Ordering::Less)
            } else {
                Some(Ordering::Equal)
            }
        }
        other => Some(other),
    }
}

/// Exact `u64` vs `f64` comparison — F-2 (#791), the unsigned analogue of
/// [`cmp_i64_f64`].
///
/// Same derivation, bounded to `[0, 2^64)` instead of `[-2^63, 2^63)`:
/// `u64::MIN == 0` and `u64::MAX == 2^64 - 1` — both `0.0` and `2^64` are
/// exact `f64` literals (`2^64` is a power of two, well within the 11-bit
/// exponent range). `f < 0.0` means `f < u64::MIN <= u`, so `u > f`.
/// `f >= 2^64` means `f >= u64::MAX + 1 > u64::MAX >= u`, so `u < f`. For
/// finite `f` in `[0.0, 2^64)`, `f.floor()` is an exact integer value in
/// that range, losslessly representable as `u64` (the same
/// magnitude-vs-mantissa argument as `cmp_i64_f64` applies: any `f64` with
/// `f >= 2^53` has no fractional bits left, so `floor`/`fract` are exact
/// there; below `2^53` they behave as normal exact-integer-valued doubles).
/// The exact-equal tie-break is identical to `cmp_i64_f64`: compare `f`
/// against `f_floor` directly (never `f.fract()`, which only matters here
/// because negative `f` is otherwise rejected first by the `f < 0.0` bound
/// -- but the same non-sign-safe caveat applies once `f` is negative and
/// non-integer, e.g. `f == -0.5` must report `u > f`, which the `f < 0.0`
/// branch already handles before reaching the tie-break).
#[inline]
pub(crate) fn cmp_u64_f64(u: u64, f: f64) -> Option<Ordering> {
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
    const U64_MIN_AS_F64: f64 = 0.0; // 0, exact
    const U64_MAX_EXCLUSIVE_UPPER_BOUND: f64 = 18446744073709551616.0; // 2^64, exact
    if f < U64_MIN_AS_F64 {
        return Some(Ordering::Greater); // u (>= 0) > f (negative)
    }
    if f >= U64_MAX_EXCLUSIVE_UPPER_BOUND {
        return Some(Ordering::Less); // u (<= u64::MAX) < f
    }
    // f is finite and within [0, 2^64) -- f.floor() is an exact integer
    // value in that range, losslessly representable as u64.
    let f_floor = f.floor();
    let f_floor_u64 = f_floor as u64;
    match u.cmp(&f_floor_u64) {
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
