//! Probe-key completeness against the exact predicate.

use crate::record_view::{scalar_ref_numeric_eq, ProbeKey, ScalarRef};
use crate::types::value::QueryValue;
use num_bigint::BigInt;
use rust_decimal::Decimal;

fn overlaps(stored: ScalarRef<'_>, query: &QueryValue) -> bool {
    let exact = scalar_ref_numeric_eq(stored, query);
    let query_keys = ProbeKey::for_query(query);
    let has_common_key = ProbeKey::for_scalar(stored)
        .iter()
        .any(|key| query_keys.contains(key));
    assert!(
        !exact || has_common_key,
        "equal pair lacks candidate key: {stored:?} / {query:?}"
    );
    has_common_key
}

#[test]
fn probe_keys_cover_predicate_equal_pairs() {
    let large = BigInt::parse_bytes(b"9007199254740993", 10).unwrap();
    let cases = [
        (ScalarRef::Int(-7), QueryValue::Int(-7)),
        (ScalarRef::F64(-0.0), QueryValue::Int(0)),
        (ScalarRef::Int(9), QueryValue::F64(9.0)),
        (ScalarRef::Str("9"), QueryValue::Big(BigInt::from(9))),
        (
            ScalarRef::Str("-9.00"),
            QueryValue::Dec(Decimal::new(-900, 2)),
        ),
        (ScalarRef::Str("1000"), QueryValue::Dec(Decimal::from(1000))),
        (
            ScalarRef::Str("9007199254740993"),
            QueryValue::Big(large.clone()),
        ),
        (ScalarRef::Str("0.50"), QueryValue::Dec(Decimal::new(50, 2))),
        (ScalarRef::Str("same"), QueryValue::Str("same".to_owned())),
        (ScalarRef::Bin(&[1, 2]), QueryValue::Bin(vec![1, 2])),
    ];
    for (stored, query) in cases {
        assert!(overlaps(stored, &query));
        assert!(scalar_ref_numeric_eq(stored, &query));
    }
}

#[test]
fn probe_keys_preserve_numeric_and_string_selectivity() {
    assert_ne!(
        ProbeKey::for_query(&QueryValue::Int(1)),
        ProbeKey::for_query(&QueryValue::Int(2))
    );
    assert_eq!(
        ProbeKey::for_query(&QueryValue::Str("x".to_owned())),
        vec![ProbeKey::String("x".to_owned())]
    );
    assert!(!overlaps(
        ScalarRef::Str("1e3"),
        &QueryValue::Dec(Decimal::from(1000))
    ));
    assert!(!overlaps(ScalarRef::Str("x"), &QueryValue::Int(1)));
    assert!(!overlaps(
        ScalarRef::Str("1.0"),
        &QueryValue::Str("1".to_owned())
    ));
    assert!(!overlaps(
        ScalarRef::F64(f64::NAN),
        &QueryValue::F64(f64::NAN)
    ));
    assert!(ProbeKey::for_scalar(ScalarRef::F64(f64::NAN)).is_empty());
}
