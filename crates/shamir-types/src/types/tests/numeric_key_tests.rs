//! Canonical numeric key tests (RV-FUNCLIB-5FA455F7, D1-S2): exact
//! Int↔F64/Big/Dec parity, -0.0 → 0, per-bit NaN, and no collapse of
//! distinct integers beyond `2^53` / outside `i64`.

use num_bigint::BigInt;
use rust_decimal::Decimal;
use std::hash::{Hash, Hasher};

use crate::types::numeric_key::{numeric_eq_values, NumericKey};
use crate::types::value::Value;

fn hash_of(key: &NumericKey) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

/// `Int(1)`, `F64(1.0)`, `Big(1)` and `Dec(1.0)`/`Dec(1)` are ONE key —
/// equal values AND identical hashes.
#[test]
fn rv_funclib_5fa455f7_numeric_key_exact_parity_hash_identically() {
    let int = NumericKey::Int(1);
    let f64_key = NumericKey::from_f64(1.0);
    let big = NumericKey::from_bigint(&BigInt::from(1));
    let dec = NumericKey::from_decimal(&Decimal::from(1)).unwrap();
    let dec_trailing = NumericKey::from_decimal(&Decimal::new(10, 1)).unwrap(); // 1.0
    assert_eq!(int, f64_key);
    assert_eq!(int, big);
    assert_eq!(int, dec);
    assert_eq!(int, dec_trailing);
    assert_eq!(hash_of(&int), hash_of(&f64_key));
    assert_eq!(hash_of(&int), hash_of(&big));
    assert_eq!(hash_of(&int), hash_of(&dec));
}

/// `-0.0` and `0.0` both key as `Int(0)`.
#[test]
fn rv_funclib_5fa455f7_numeric_key_signed_zero_keys_as_zero() {
    assert_eq!(NumericKey::from_f64(-0.0), NumericKey::Int(0));
    assert_eq!(NumericKey::from_f64(0.0), NumericKey::Int(0));
    assert_eq!(NumericKey::from_f64(-0.0), NumericKey::from_f64(0.0));
}

/// EXACT Int↔Big: `Int(2^53+1)` ≠ `Big(2^53)`; Bigs outside `i64` stay
/// distinct; the `i64` boundary values are exact.
#[test]
fn rv_funclib_5fa455f7_numeric_key_exact_int_big_beyond_2p53_and_i64() {
    assert_ne!(
        NumericKey::Int(9007199254740993),
        NumericKey::from_bigint(&BigInt::from(9007199254740992i64))
    );
    // Beyond i64: distinct Bigs, distinct keys; no f64 rounding collapse
    // (2^63 and 2^63 + 1 round to the SAME f64).
    let big_a = NumericKey::from_bigint(&(BigInt::from(1u8) << 63usize));
    let big_b = NumericKey::from_bigint(&((BigInt::from(1u8) << 63usize) + BigInt::from(1u8)));
    assert_ne!(big_a, big_b);
    assert_ne!(hash_of(&big_a), hash_of(&big_b));
    // i64 boundaries.
    assert_eq!(
        NumericKey::from_bigint(&BigInt::from(i64::MAX)),
        NumericKey::Int(i64::MAX)
    );
    assert_eq!(
        NumericKey::from_bigint(&BigInt::from(i64::MIN)),
        NumericKey::Int(i64::MIN)
    );
    assert_ne!(
        NumericKey::from_bigint(&(BigInt::from(i64::MIN) - BigInt::from(1u8))),
        NumericKey::Int(i64::MIN)
    );
}

/// NaN keys per its own bit pattern. This is a KEY identity used by index
/// hashing and the canonical class maps — it does NOT mean two NaN rows
/// deduplicate together: occurrence-DISTINCTNESS of NaN is enforced by the
/// DISTINCT/GROUP BY consumers (`apply_distinct_qv` keeps every top-level
/// NaN leaf; GROUP BY emits occurrence-unique `NanOnce` keys), and NaN
/// never matches an FK probe. Two different NaN payloads are different keys.
#[test]
fn rv_funclib_5fa455f7_numeric_key_nan_per_bit_pattern() {
    let quiet = NumericKey::from_f64(f64::NAN);
    let payload = NumericKey::from_f64(f64::from_bits(0x7ff8_0000_0000_0001));
    assert_eq!(quiet, NumericKey::from_f64(f64::NAN));
    assert_ne!(quiet, payload);
    assert_ne!(quiet, NumericKey::Int(0));
    assert_ne!(quiet, NumericKey::F64Bits(0.0f64.to_bits()));
    // ±inf and non-integral floats keep their bit key.
    assert_eq!(
        NumericKey::from_f64(f64::INFINITY),
        NumericKey::F64Bits(f64::INFINITY.to_bits())
    );
    assert_eq!(
        NumericKey::from_f64(0.5),
        NumericKey::F64Bits(0.5f64.to_bits())
    );
}

/// A non-integral `Dec` has NO integer key (`None`) — exact-only equality.
#[test]
fn rv_funclib_5fa455f7_numeric_key_dec_exact_only() {
    assert_eq!(NumericKey::from_decimal(&Decimal::new(5, 1)), None); // 0.5
    assert_eq!(
        NumericKey::from_decimal(&Decimal::from(-7)),
        Some(NumericKey::Int(-7))
    );
}

/// A `Dec` integral outside `i64` (2^63) and the equal `Big` BOTH map to
/// the same `BigBits` key, with identical hashes. This is the DISTINCT
/// panic fix: round 1's `num_class` mapped `Dec(2^63)` to `None` (claiming
/// `from_decimal` "only returns Int"), so the leaf fell through to
/// `hash_qv`'s `unreachable!` and panicked.
#[test]
fn rv_funclib_5fa455f7_numeric_key_dec_big_bits_2p63_parity() {
    let two_pow_63 = 9223372036854775808i128; // 2^63 — outside i64
    let dec = Decimal::from_i128_with_scale(two_pow_63, 0);
    let big = BigInt::from(two_pow_63);
    let dec_key = NumericKey::from_decimal(&dec).expect("integral Dec has an integer key");
    let big_key = NumericKey::from_bigint(&big);
    assert_eq!(
        dec_key,
        NumericKey::BigBits {
            negative: false,
            magnitude_be: vec![128u8, 0, 0, 0, 0, 0, 0, 0].into()
        }
    );
    assert_eq!(dec_key, big_key);
    assert_eq!(hash_of(&dec_key), hash_of(&big_key));
}
/// The FULL equality table implied by the ONE rule (round 3): all forms of
/// the same integer — `Int`, `Big`, `Dec`, integral `F64` (ANY magnitude,
/// including `F64(2^63)`) — are mutually equal via `numeric_eq_values`;
/// `F64(0.5) != Dec(0.5)` (own key classes); NaN is never equal to
/// anything. Replaces the deleted round-2 exact-binary helpers
/// (`f64_eq_decimal`/`f64_eq_bigint`/`f64_to_exact_decimal`).
#[test]
fn rv_funclib_5fa455f7_numeric_key_equality_transitivity_table() {
    // Every exact form of each integer value (Int omitted for 2^63 — it
    // does not fit `i64`; F64(2^63) still canonicalises to BigBits).
    let integers: Vec<(i128, Vec<Value<()>>)> = vec![
        (
            0,
            vec![
                Value::Int(0),
                Value::F64(0.0),
                Value::F64(-0.0),
                Value::Dec(Decimal::from(0)),
                Value::Big(BigInt::from(0)),
            ],
        ),
        (
            1,
            vec![
                Value::Int(1),
                Value::F64(1.0),
                Value::Dec(Decimal::from(1)),
                Value::Big(BigInt::from(1)),
            ],
        ),
        (
            1 << 53,
            vec![
                Value::Int(1 << 53),
                Value::F64(9_007_199_254_740_992.0),
                Value::Dec(Decimal::from(9_007_199_254_740_992i64)),
                Value::Big(BigInt::from(9_007_199_254_740_992i64)),
            ],
        ),
        (
            1 << 63,
            vec![
                Value::F64(9_223_372_036_854_775_808.0),
                Value::Dec(Decimal::from_i128_with_scale(1 << 63, 0)),
                Value::Big(BigInt::from(1 << 63)),
            ],
        ),
    ];
    // Same integer: every pair of forms is EQUAL (full transitivity table).
    for (_, forms) in &integers {
        for a in forms {
            for b in forms {
                assert!(
                    numeric_eq_values(a, b),
                    "forms of the same integer must be equal: {a:?} vs {b:?}"
                );
            }
        }
    }
    // Different integers: cross pairs are NOT equal — including
    // F64(2^53) vs Big(2^53+1) (the lossy `as f64` collapse).
    let big_2p53_plus_1 = Value::<()>::Big(BigInt::from((1i64 << 53) + 1));
    assert!(!numeric_eq_values(
        &Value::F64(9_007_199_254_740_992.0),
        &big_2p53_plus_1
    ));
    for (i, forms) in &integers {
        for (j, other) in &integers {
            if i == j {
                continue;
            }
            assert!(
                !numeric_eq_values(&forms[0], &other[0]),
                "distinct integers must not be equal: {i} vs {j}"
            );
        }
    }
    // Non-integral F64 vs non-integral Dec: separate key classes (the
    // lossy F64↔Dec comparison is ORDERING-only).
    assert!(!numeric_eq_values(
        &Value::<()>::F64(0.5),
        &Value::Dec(Decimal::new(5, 1))
    ));
    // NaN is never equal to anything, itself included.
    let nan = Value::<()>::F64(f64::NAN);
    for v in [
        Value::<()>::Int(0),
        Value::F64(0.0),
        Value::F64(f64::NAN),
        Value::Dec(Decimal::from(0)),
        Value::Big(BigInt::from(0)),
    ] {
        assert!(!numeric_eq_values(&nan, &v));
        assert!(!numeric_eq_values(&v, &nan));
    }
    // Equality consistency with the raw NumericKey classes: numeric_eq
    // agrees with key equality for every mixed-type pair.
    let pairs = [
        (Value::<()>::F64(1.0), Value::Dec(Decimal::from(1)), true),
        (Value::F64(-0.0), Value::Int(0), true),
        (
            Value::F64(9_223_372_036_854_775_808.0),
            Value::Big(BigInt::from(1 << 63)),
            true,
        ),
        (Value::F64(0.5), Value::Dec(Decimal::new(5, 1)), false),
    ];
    for (a, b, expect) in &pairs {
        assert_eq!(numeric_eq_values(a, b), *expect, "{a:?} vs {b:?}");
    }
}
