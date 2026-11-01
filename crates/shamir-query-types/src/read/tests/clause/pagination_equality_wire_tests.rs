use num_bigint::BigInt;
use std::str::FromStr;

use shamir_collections::TMap;

use shamir_types::types::value::QueryValue;

use crate::read::clause::limit::Pagination;
use crate::testing_alloc::count_allocations;

fn after(key: Vec<QueryValue>) -> Pagination {
    Pagination::after(key, None)
}

#[test]
fn pagination_after_equality_preserves_wire_representations() {
    let left = after(vec![QueryValue::Int(1), QueryValue::F64(0.0)]);
    let same = after(vec![QueryValue::Int(1), QueryValue::F64(0.0)]);
    let different_float_bits = after(vec![QueryValue::Int(1), QueryValue::F64(-0.0)]);
    assert_eq!(left, same);
    assert_ne!(left, different_float_bits);

    let mut first = TMap::default();
    first.insert("a".to_owned(), QueryValue::Int(1));
    first.insert("b".to_owned(), QueryValue::Int(2));
    let mut reversed = TMap::default();
    reversed.insert("b".to_owned(), QueryValue::Int(2));
    reversed.insert("a".to_owned(), QueryValue::Int(1));
    assert_ne!(
        after(vec![QueryValue::Map(first)]),
        after(vec![QueryValue::Map(reversed)])
    );
}

#[test]
fn pagination_after_big_cross_representations_preserve_wire_semantics() {
    let big = QueryValue::Big(BigInt::from_str("9223372036854775808").unwrap());
    let same = QueryValue::Str("9223372036854775808".into());
    assert_eq!(after(vec![big]), after(vec![same]));
    assert_ne!(
        after(vec![QueryValue::Big(
            BigInt::from_str("9223372036854775808").unwrap()
        )]),
        after(vec![QueryValue::Str("09223372036854775808".into())])
    );
}

fn big(text: &str) -> QueryValue {
    QueryValue::Big(BigInt::from_str(text).unwrap())
}

fn decimal(text: &str) -> QueryValue {
    QueryValue::Dec(text.parse().unwrap())
}

fn text(value: &str) -> QueryValue {
    QueryValue::Str(value.into())
}

// 2^127 and -2^127 - 1 sit just outside i128; both have 39 digits.
const WIDE_POSITIVE: &str = "170141183460469231731687303715884105728";
const WIDE_NEGATIVE: &str = "-170141183460469231731687303715884105729";

#[test]
fn pagination_after_big_cross_comparisons_do_not_allocate() {
    let pairs = [
        (
            big("9223372036854775808"),
            text("9223372036854775808"),
            true,
        ),
        (
            big("-9223372036854775809"),
            text("-9223372036854775809"),
            true,
        ),
        (big("0"), text("0"), true),
        (big("0"), text("-0"), false),
        (
            big("9223372036854775808"),
            text("9223372036854775809"),
            false,
        ),
        (
            big("9223372036854775808"),
            text("09223372036854775808"),
            false,
        ),
        (big("123"), decimal("123"), true),
        (big("123"), decimal("123.0"), false),
        (big("-123"), decimal("-123"), true),
        (
            big(WIDE_POSITIVE),
            decimal("79228162514264337593543950335"),
            false,
        ),
        (
            big(WIDE_POSITIVE),
            text("-170141183460469231731687303715884105728"),
            false,
        ),
        (
            big(WIDE_POSITIVE),
            text("17014118346046923173168730371588410572"),
            false,
        ),
        (
            big(WIDE_POSITIVE),
            text("1701411834604692317316873037158841057280"),
            false,
        ),
        (
            big(WIDE_POSITIVE),
            text("17014118346046923173168730371588410572a"),
            false,
        ),
        (
            big(WIDE_POSITIVE),
            text("070141183460469231731687303715884105728"),
            false,
        ),
        (big(WIDE_NEGATIVE), text(WIDE_POSITIVE), false),
    ];
    for (left, right, expected) in pairs {
        let (left, right) = (after(vec![left]), after(vec![right]));
        let (allocations, (forward, backward)) =
            count_allocations(|| (left == right, right == left));
        assert_eq!(allocations, 0, "comparison must not allocate");
        assert_eq!((forward, backward), (expected, expected));
    }
}

#[test]
fn pagination_after_wide_big_candidate_still_compares_exactly() {
    let wide = after(vec![big(WIDE_POSITIVE)]);
    assert_eq!(wide, after(vec![text(WIDE_POSITIVE)]));
    assert_ne!(
        wide,
        after(vec![text("170141183460469231731687303715884105729")])
    );
    assert_eq!(
        after(vec![big(WIDE_NEGATIVE)]),
        after(vec![text(WIDE_NEGATIVE)])
    );
}
