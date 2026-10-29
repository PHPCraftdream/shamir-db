use crate::compare::compare;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue as V;
use std::cmp::Ordering::{Equal, Greater, Less};

fn decimal(s: &str) -> V {
    V::Dec(Decimal::from_str_exact(s).unwrap())
}

#[test]
fn exact_boundary_triples_and_transitivity() {
    let values = [
        V::Int(9_007_199_254_740_992),
        V::F64(9_007_199_254_740_992.0),
        V::Int(9_007_199_254_740_993),
    ];
    assert_eq!(compare(&values[0], &values[1]), Equal);
    assert_eq!(compare(&values[1], &values[2]), Less);
    assert_eq!(compare(&values[2], &values[0]), Greater);
    for a in &values {
        for b in &values {
            for c in &values {
                if compare(a, b) != Greater && compare(b, c) != Greater {
                    assert_ne!(compare(a, c), Greater);
                }
            }
        }
    }
}

#[test]
fn decimal_float_binary_boundaries_and_subnormals() {
    assert_eq!(compare(&decimal("0.1"), &V::F64(0.1)), Less);
    assert_eq!(compare(&decimal("-0.1"), &V::F64(-0.1)), Greater);
    assert_eq!(compare(&decimal("0.125"), &V::F64(0.125)), Equal);
    assert_eq!(compare(&V::F64(f64::from_bits(1)), &V::Int(0)), Greater);
    assert_eq!(compare(&V::F64(-0.0), &decimal("0.000")), Equal);
    assert_eq!(
        compare(
            &decimal("79228162514264337593543950335"),
            &V::Big(BigInt::from(1_u8) << 96_usize)
        ),
        Less
    );
}

#[test]
fn huge_integers_are_finite_and_nan_is_last() {
    let huge = BigInt::from(1_u8) << 2000_usize;
    assert_eq!(compare(&V::Big(huge.clone()), &V::F64(f64::INFINITY)), Less);
    assert_eq!(compare(&V::Big(-huge), &V::F64(f64::NEG_INFINITY)), Greater);
    assert_eq!(compare(&V::F64(f64::INFINITY), &V::F64(f64::NAN)), Less);
    assert_eq!(
        compare(
            &V::F64(f64::NAN),
            &V::F64(f64::from_bits(0xfff8_0000_0000_0001))
        ),
        Equal
    );
}
