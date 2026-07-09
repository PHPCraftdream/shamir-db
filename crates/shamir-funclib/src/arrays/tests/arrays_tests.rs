//! Per-function `/arrays` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::arrays;
use crate::registry::{v_bool, v_dec, v_int, v_list, v_str, ScalarRegistry};
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    arrays::register(&mut r);
    r
}

fn dec(s: &str) -> QueryValue {
    QueryValue::Dec(Decimal::from_str_exact(s).unwrap())
}

fn list(items: Vec<QueryValue>) -> QueryValue {
    v_list(items)
}

fn ints(xs: &[i64]) -> QueryValue {
    v_list(xs.iter().map(|&n| QueryValue::Int(n)).collect())
}

fn strs(xs: &[&str]) -> QueryValue {
    v_list(xs.iter().map(|&s| QueryValue::Str(s.into())).collect())
}

#[test]
fn length_ok_and_type_error() {
    let r = reg();
    assert_eq!(r.call("length", &[ints(&[1, 2, 3])]).unwrap(), v_int(3));
    assert_eq!(r.call("length", &[list(vec![])]).unwrap(), v_int(0));
    // error: not a list
    assert_eq!(
        r.call("length", &[QueryValue::Int(7)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn get_ok_and_out_of_range() {
    let r = reg();
    assert_eq!(
        r.call("get", &[ints(&[10, 20, 30]), QueryValue::Int(1)])
            .unwrap(),
        QueryValue::Int(20)
    );
    // error: index past end
    assert_eq!(
        r.call("get", &[ints(&[10]), QueryValue::Int(5)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
    // error: negative index
    assert_eq!(
        r.call("get", &[ints(&[10]), QueryValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn slice_ok_and_clamped_and_neg() {
    let r = reg();
    assert_eq!(
        r.call(
            "slice",
            &[
                ints(&[1, 2, 3, 4, 5]),
                QueryValue::Int(1),
                QueryValue::Int(2)
            ]
        )
        .unwrap(),
        ints(&[2, 3])
    );
    // len past end is clamped to the array tail
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), QueryValue::Int(2), QueryValue::Int(99)]
        )
        .unwrap(),
        ints(&[3])
    );
    // start past end -> empty
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), QueryValue::Int(10), QueryValue::Int(2)]
        )
        .unwrap(),
        list(vec![])
    );
    // error: negative length
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), QueryValue::Int(0), QueryValue::Int(-1)]
        )
        .unwrap_err()
        .code,
        "out_of_range"
    );
}

#[test]
fn contains_ok_and_missing() {
    let r = reg();
    assert_eq!(
        r.call("contains", &[ints(&[1, 2, 3]), QueryValue::Int(2)])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("contains", &[ints(&[1, 2, 3]), QueryValue::Int(9)])
            .unwrap(),
        v_bool(false)
    );
    // error: needle missing -> arity
    assert_eq!(r.call("contains", &[ints(&[1])]).unwrap_err().code, "arity");
}

#[test]
fn index_of_found_and_absent() {
    let r = reg();
    assert_eq!(
        r.call(
            "index_of",
            &[strs(&["a", "b", "c"]), QueryValue::Str("b".into())]
        )
        .unwrap(),
        v_int(1)
    );
    // absent -> -1
    assert_eq!(
        r.call("index_of", &[strs(&["a"]), QueryValue::Str("z".into())])
            .unwrap(),
        v_int(-1)
    );
    // error: first arg not a list
    assert_eq!(
        r.call("index_of", &[QueryValue::Int(1), QueryValue::Int(1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn first_last_ok_and_empty() {
    let r = reg();
    assert_eq!(
        r.call("first", &[ints(&[7, 8, 9])]).unwrap(),
        QueryValue::Int(7)
    );
    assert_eq!(
        r.call("last", &[ints(&[7, 8, 9])]).unwrap(),
        QueryValue::Int(9)
    );
    // error: empty array
    assert_eq!(r.call("first", &[list(vec![])]).unwrap_err().code, "empty");
    assert_eq!(r.call("last", &[list(vec![])]).unwrap_err().code, "empty");
}

#[test]
fn flatten_ok_and_non_list_element() {
    let r = reg();
    let nested = list(vec![ints(&[1, 2]), ints(&[3]), list(vec![])]);
    assert_eq!(r.call("flatten", &[nested]).unwrap(), ints(&[1, 2, 3]));
    // error: element is not a list
    let bad = list(vec![ints(&[1]), QueryValue::Int(2)]);
    assert_eq!(r.call("flatten", &[bad]).unwrap_err().code, "type_mismatch");
}

#[test]
fn distinct_preserves_first_order() {
    let r = reg();
    assert_eq!(
        r.call("distinct", &[ints(&[1, 2, 2, 3, 1, 3])]).unwrap(),
        ints(&[1, 2, 3])
    );
    // edge: empty stays empty
    assert_eq!(r.call("distinct", &[list(vec![])]).unwrap(), list(vec![]));
    // error: not a list
    assert_eq!(
        r.call("distinct", &[QueryValue::Bool(true)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn distinct_all_dupes_and_no_dupes() {
    let r = reg();
    // All dupes → single first occurrence.
    assert_eq!(
        r.call("distinct", &[ints(&[7, 7, 7, 7, 7])]).unwrap(),
        ints(&[7])
    );
    // No dupes → unchanged, first-order preserved.
    assert_eq!(
        r.call("distinct", &[ints(&[5, 3, 9, 1, 4])]).unwrap(),
        ints(&[5, 3, 9, 1, 4])
    );
    // Single element.
    assert_eq!(r.call("distinct", &[ints(&[42])]).unwrap(), ints(&[42]));
}

#[test]
fn distinct_mixed_types_first_order() {
    let r = reg();
    // Mixed-type dedup: each distinct `QueryValue` kept on first sight,
    // regardless of variant. (Distinct discriminants never collide.)
    let input = list(vec![
        QueryValue::Int(1),
        QueryValue::Str("a".into()),
        QueryValue::Int(1),
        QueryValue::Bool(true),
        QueryValue::Str("a".into()),
        QueryValue::Null,
        QueryValue::Bool(true),
        QueryValue::Int(2),
    ]);
    let expected = list(vec![
        QueryValue::Int(1),
        QueryValue::Str("a".into()),
        QueryValue::Bool(true),
        QueryValue::Null,
        QueryValue::Int(2),
    ]);
    assert_eq!(r.call("distinct", &[input]).unwrap(), expected);
}

#[test]
fn distinct_nan_same_bit_pattern() {
    let r = reg();
    // Two NaN values with the SAME bit pattern hash-equal AND PartialEq-equal,
    // so they are deduped exactly as the legacy linear-scan did.
    let nan = f64::from_bits(0x7FF8000000000001u64);
    let input = list(vec![
        QueryValue::F64(nan),
        QueryValue::F64(1.5),
        QueryValue::F64(nan),
    ]);
    let out = r.call("distinct", &[input]).unwrap();
    let arr = out.as_array().expect("list out");
    assert_eq!(arr.len(), 2, "two distinct F64 values survive");
}

#[test]
fn distinct_nan_different_bit_patterns_still_dedup() {
    let r = reg();
    // `PartialEq` treats ALL NaN as equal regardless of bit pattern (see
    // `impl PartialEq for Value`), so two NaN values with DIFFERENT bit
    // patterns must still collapse to one under `distinct()`, matching the
    // legacy O(N²) `==`-based behavior. Regression test for the Hash/Eq
    // consistency bug found during @sh review of this fix: `Hash` used to
    // hash the raw bit pattern, so differently-NaN-bit-patterned values
    // hashed into different HashSet buckets and both survived distinct()
    // even though `PartialEq` said they were equal.
    let nan_a = f64::from_bits(0x7FF8000000000001u64);
    let nan_b = f64::from_bits(0x7FF8000000000002u64);
    let input = list(vec![
        QueryValue::F64(nan_a),
        QueryValue::F64(1.5),
        QueryValue::F64(nan_b),
    ]);
    let out = r.call("distinct", &[input]).unwrap();
    let arr = out.as_array().expect("list out");
    assert_eq!(
        arr.len(),
        2,
        "differing-bit-pattern NaN values must still dedup to one, \
         matching PartialEq semantics"
    );
}

#[test]
fn distinct_large_unique_matches_naive() {
    let r = reg();
    // Reference: the legacy O(N²) path returns first-sight uniques. The
    // hash-based path must match it element-for-element on a large unique
    // input (worst case for the old code).
    let n = 500i64;
    let input: Vec<QueryValue> = (0..n).map(QueryValue::Int).collect();
    let out = r.call("distinct", &[list(input.clone())]).unwrap();
    let arr = out.as_array().expect("list out");
    // Every element appears exactly once, in original order.
    assert_eq!(arr.len(), n as usize);
    for (i, v) in arr.iter().enumerate() {
        assert_eq!(*v, input[i]);
    }
}

#[test]
fn sort_numeric_and_non_numeric_error() {
    let r = reg();
    assert_eq!(
        r.call("sort", &[ints(&[3, 1, 2, -5])]).unwrap(),
        ints(&[-5, 1, 2, 3])
    );
    // error: non-numeric element
    assert_eq!(
        r.call("sort", &[strs(&["a", "b"])]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn join_ok_and_non_string_element() {
    let r = reg();
    assert_eq!(
        r.call(
            "join",
            &[strs(&["a", "b", "c"]), QueryValue::Str("-".into())]
        )
        .unwrap(),
        v_str("a-b-c".into())
    );
    // edge: empty array -> empty string
    assert_eq!(
        r.call("join", &[list(vec![]), QueryValue::Str(",".into())])
            .unwrap(),
        v_str(String::new())
    );
    // error: non-string element
    assert_eq!(
        r.call("join", &[ints(&[1, 2]), QueryValue::Str(",".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn sum_min_max_avg_and_empty() {
    let r = reg();
    assert_eq!(r.call("sum", &[ints(&[1, 2, 3, 4])]).unwrap(), dec("10"));
    // min/max now return elements as-is (Int, not coerced Dec).
    assert_eq!(
        r.call("min", &[ints(&[5, 2, 8, -1])]).unwrap(),
        QueryValue::Int(-1)
    );
    assert_eq!(
        r.call("max", &[ints(&[5, 2, 8, -1])]).unwrap(),
        QueryValue::Int(8)
    );
    assert_eq!(
        r.call("avg", &[ints(&[2, 4, 6])]).unwrap(),
        v_dec(Decimal::from(4))
    );
    // error: empty array
    assert_eq!(r.call("sum", &[list(vec![])]).unwrap_err().code, "empty");
    assert_eq!(r.call("avg", &[list(vec![])]).unwrap_err().code, "empty");
    assert_eq!(r.call("min", &[list(vec![])]).unwrap_err().code, "empty");
    assert_eq!(r.call("max", &[list(vec![])]).unwrap_err().code, "empty");
}

#[test]
fn min_max_cross_type_elements() {
    let r = reg();
    // Mixed types in a list: Bool true (rank 1) < Int 5 (rank 2) < Str "a" (rank 3).
    let mixed = list(vec![
        QueryValue::Bool(true),
        QueryValue::Int(5),
        QueryValue::Str("a".into()),
    ]);
    assert_eq!(
        r.call("min", std::slice::from_ref(&mixed)).unwrap(),
        QueryValue::Bool(true)
    );
    assert_eq!(
        r.call("max", std::slice::from_ref(&mixed)).unwrap(),
        QueryValue::Str("a".into())
    );
    // max no longer errors on strings — it uses cross-type compare.
    assert_eq!(
        r.call("max", &[strs(&["x"])]).unwrap(),
        QueryValue::Str("x".into())
    );
}
