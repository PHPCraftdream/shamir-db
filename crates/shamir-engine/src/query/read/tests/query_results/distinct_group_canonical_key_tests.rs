//! DISTINCT / GROUP BY canonical numeric key tests (RV-FUNCLIB-5FA455F7,
//! D1-S2): both consumers must bucket values by the SAME exact canonical
//! key as the index and the scan filter — `Int(1)`, `F64(1.0)`, `-0.0`
//! and an equal `Big`/`Dec` share one class; exact Int↔Big never through
//! `f64`; each NaN bit pattern is its own class.

use bytes::Bytes;
use num_bigint::BigInt;
use rust_decimal::Decimal;

use super::select_order_page::helpers::{intern, to_bytes_records};
use crate::query::filter::eval_context::FilterContext;
use crate::query::read::aggregation::accumulator::{group_key_item_complex, GroupKeyItem};
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::common::new_map;
use shamir_types::types::numeric_key::NumericKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

/// Mixed numeric corpus (per the card): `Int(1)`, `F64(1.0)`, `Big(1)`,
/// `F64(-0.0)`, `Int(0)`, `Int(2^53+1)`, `NaN`, `NaN`.
#[test]
fn rv_funclib_5fa455f7_distinct_mixed_numeric_corpus() {
    let rows = vec![
        QueryValue::Int(1),
        QueryValue::F64(1.0),
        QueryValue::Big(BigInt::from(1)),
        QueryValue::F64(-0.0),
        QueryValue::Int(0),
        QueryValue::Int(9007199254740993), // 2^53+1, NOT exactly an f64
        QueryValue::F64(f64::NAN),
        QueryValue::F64(f64::NAN),
    ];
    let out = apply_distinct_qv(rows);
    // Classes: {1, 1.0, Big(1)}, {0, -0.0}, {2^53+1}, {NaN}, {NaN} → 5.
    // (The old lossy canonical form kept 7: Int(1)/F64(1.0)/Big(1) were
    // three separate classes and -0.0 ≠ 0.)
    assert_eq!(out.len(), 5);
}

/// `Int(2^53+1)` and an exactly-equal `Big` must dedup together, while a
/// `Big` differing by 1 must stay distinct (exact — never through f64).
#[test]
fn rv_funclib_5fa455f7_distinct_exact_int_big_beyond_2p53() {
    let out = apply_distinct_qv(vec![
        QueryValue::Int(9007199254740993),
        QueryValue::Big(BigInt::from(9007199254740993i64)),
        QueryValue::Big(BigInt::from(9007199254740992i64)),
    ]);
    assert_eq!(out.len(), 2);
}

/// GROUP BY over the mixed corpus, through `to_bytes_records` (the stored
/// wire form): `Big(1)` serialises as its decimal STRING until D2, so it
/// reads back as `Str("1")` and forms its OWN group. Hand recount of the
/// 8-row corpus: {Int(1), F64(1.0)} → 2, {Str("1")} → 1, {F64(-0.0),
/// Int(0)} → 2, {2^53+1} → 1, {NaN} → 1, {NaN} → 1 — SIX groups with
/// counts [2,1,2,1,1,1] (the brief's [3,...] double-counted Big(1), which
/// is the Str group, not part of the 1-class).
#[test]
fn rv_funclib_5fa455f7_group_by_mixed_numeric_corpus() {
    let interner = Interner::default();
    let v_id = intern(&interner, "v");
    let rows: Vec<(RecordId, InnerValue)> = [
        InnerValue::Int(1),
        InnerValue::F64(1.0),
        InnerValue::Big(BigInt::from(1)),
        InnerValue::F64(-0.0),
        InnerValue::Int(0),
        InnerValue::Int(9007199254740993),
        InnerValue::F64(f64::NAN),
        InnerValue::F64(f64::NAN),
    ]
    .into_iter()
    .map(|v| {
        let mut map = new_map();
        map.insert(InternerKey::new(v_id), v);
        (RecordId::new(), InnerValue::Map(map))
    })
    .collect();

    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let group_by = GroupBy::new(["v"]);
    let select = Select {
        items: vec![select::field("v"), select::count_all("cnt")],
        distinct: false,
    };
    let records: Vec<(RecordId, Bytes)> = to_bytes_records(&rows);
    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 6, "six canonical classes over stored bytes");
    let total: i64 = result
        .iter()
        .map(|r| match &r["cnt"] {
            QueryValue::Int(n) => *n,
            _ => 0,
        })
        .sum();
    assert_eq!(total, 8, "per-group counts must cover the whole corpus");
    // MEMBERSHIP: group order is the sorted key order, so look each group
    // up by its key value instead of by position.
    let count_of = |pred: &dyn Fn(&QueryValue) -> bool| -> Vec<i64> {
        result
            .iter()
            .filter(|r| pred(&r["v"]))
            .map(|r| match &r["cnt"] {
                QueryValue::Int(n) => *n,
                other => panic!("cnt must be Int, got {other:?}"),
            })
            .collect()
    };
    // {Int(1), F64(1.0)} -> one group of 2
    assert_eq!(
        count_of(
            &|v| matches!(v, QueryValue::Int(1)) || matches!(v, QueryValue::F64(f) if *f == 1.0)
        ),
        vec![2]
    );
    // stored Big(1) is the string "1" until D2
    assert_eq!(count_of(&|v| *v == QueryValue::Str("1".into())), vec![1]);
    // {-0.0, 0} -> one zero group of 2 (reported as the first member seen)
    assert_eq!(
        count_of(
            &|v| matches!(v, QueryValue::F64(f) if *f == 0.0) || matches!(v, QueryValue::Int(0))
        ),
        vec![2]
    );
    assert_eq!(
        count_of(&|v| *v == QueryValue::Int(9_007_199_254_740_993)),
        vec![1]
    );
    // each NaN occurrence is its own group
    assert_eq!(
        count_of(&|v| matches!(v, QueryValue::F64(f) if f.is_nan())),
        vec![1, 1]
    );
}

/// Integral `Dec(1)`/`Big(1)` and `Dec(2^63)`/`Big(2^63)` key through
/// `group_key_item_complex` as ONE canonical numeric class each
/// (`Num(Int(1))` and the same `BigBits` key) — unit test on the
/// complex-group-key builder itself.
#[test]
fn rv_funclib_5fa455f7_group_key_item_complex_dec_big_numeric_classes() {
    let interner = Interner::default();
    let one = GroupKeyItem::Num(NumericKey::Int(1));
    assert_eq!(
        group_key_item_complex(&InnerValue::Dec(Decimal::from(1)), &interner),
        one
    );
    assert_eq!(
        group_key_item_complex(&InnerValue::Big(BigInt::from(1)), &interner),
        one
    );
    let two_pow_63 = 1i128 << 63;
    let dec_key = group_key_item_complex(
        &InnerValue::Dec(Decimal::from_i128_with_scale(two_pow_63, 0)),
        &interner,
    );
    let big_key = group_key_item_complex(&InnerValue::Big(BigInt::from(two_pow_63)), &interner);
    assert!(matches!(
        dec_key,
        GroupKeyItem::Num(NumericKey::BigBits { .. })
    ));
    assert_eq!(dec_key, big_key);
}

/// A byte-serialised `Big(1)` groups WITH `Str("1")` — pins the pre-D2
/// storage projection (stored `Big`/`Dec` are decimal strings, so the
/// group-by-over-stored-bytes path sees strings until D2).
#[test]
fn rv_funclib_5fa455f7_group_by_stored_big_groups_as_string_until_d2() {
    let interner = Interner::default();
    let v_id = intern(&interner, "v");
    let mk = |v: InnerValue| {
        let mut map = new_map();
        map.insert(InternerKey::new(v_id), v);
        (RecordId::new(), InnerValue::Map(map))
    };
    let rows: Vec<(RecordId, InnerValue)> = vec![
        mk(InnerValue::Big(BigInt::from(1))),
        mk(InnerValue::Str("1".into())),
    ];
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let group_by = GroupBy::new(["v"]);
    let select = Select {
        items: vec![select::field("v"), select::count_all("cnt")],
        distinct: false,
    };
    let records: Vec<(RecordId, Bytes)> = to_bytes_records(&rows);
    let result = apply_group_by(&records, &group_by, &select, &interner, &ctx);
    assert_eq!(result.len(), 1, "stored Big(1) groups with Str(\"1\")");
    assert_eq!(result[0]["cnt"], 2_i64);
    assert!(matches!(&result[0]["v"], QueryValue::Str(s) if s == "1"));
}

/// Non-integral `F64` never deduplicates against a `Dec`/`Big` — the
/// lossy `F64`↔`Dec`/`Big` comparison is ORDERING-only (S3); equality is
/// exact-only. An integral `Dec` still dedups with its exact `Int`.
#[test]
fn rv_funclib_5fa455f7_distinct_dec_exact_only() {
    let half = Decimal::new(5, 1); // 0.5
    let one = Decimal::from(1);
    let out = apply_distinct_qv(vec![
        QueryValue::F64(0.5),
        QueryValue::Dec(half),
        QueryValue::Big(BigInt::from(0)),
        QueryValue::Dec(one),
        QueryValue::Int(1),
    ]);
    // {F64(0.5)}, {Dec(0.5)}, {Big(0)}, {Dec(1), Int(1)} → 4.
    assert_eq!(out.len(), 4);
}

/// `Decimal(2^63)` and `Big(2^63)` are ONE DISTINCT class — no panic. This
/// pins the round-1 `unreachable!` fix: `num_class` MUST map an integral
/// `Dec` outside `i64` to the `BigBits` class (round 1 mapped it to `None`
/// and the leaf fell through to `hash_qv`'s `unreachable!`).
#[test]
fn rv_funclib_5fa455f7_distinct_dec_big_2p63_one_class_no_panic() {
    let two_pow_63 = 1i128 << 63;
    let out = apply_distinct_qv(vec![
        QueryValue::Dec(Decimal::from_i128_with_scale(two_pow_63, 0)),
        QueryValue::Big(BigInt::from(two_pow_63)),
        QueryValue::F64(9_223_372_036_854_775_808.0), // 2^63, integral F64
    ]);
    assert_eq!(out.len(), 1);
}

/// `F64(0.5)` and `Dec(0.5)` stay SEPARATE DISTINCT classes — the ONE
/// rule: a non-integral `F64` and a non-integral `Dec` keep their own key
/// classes (the lossy F64↔Dec comparison is ordering-only).
#[test]
fn rv_funclib_5fa455f7_distinct_f64_dec_nonintegral_separate() {
    let out = apply_distinct_qv(vec![
        QueryValue::F64(0.5),
        QueryValue::Dec(Decimal::new(5, 1)), // 0.5
    ]);
    assert_eq!(out.len(), 2);
}

/// Two Map rows each containing a NaN leaf at the same path do NOT merge —
/// per-occurrence NaN dedup works at ANY depth (NaN != NaN under the ONE
/// rule; each NaN leaf gets an occurrence-unique key).
#[test]
fn rv_funclib_5fa455f7_distinct_nested_nan_per_occurrence() {
    let mk = |f: f64| {
        let mut inner = new_map();
        inner.insert("x".to_string(), QueryValue::F64(f));
        QueryValue::Map(inner)
    };
    let out = apply_distinct_qv(vec![mk(f64::NAN), mk(f64::NAN)]);
    assert_eq!(
        out.len(),
        2,
        "NaN at the same nested path stays per-occurrence"
    );
    // Sanity: identical non-NaN containers still dedup.
    let out = apply_distinct_qv(vec![mk(1.0), mk(1.0)]);
    assert_eq!(out.len(), 1);
}
