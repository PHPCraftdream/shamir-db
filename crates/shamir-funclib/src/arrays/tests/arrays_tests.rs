//! Per-function `/arrays` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::arrays;
use crate::registry::{v_bool, v_dec, v_int, v_list, v_str, ScalarRegistry};
use rust_decimal::Decimal;
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    arrays::register(&mut r);
    r
}

fn dec(s: &str) -> InnerValue {
    InnerValue::Dec(Decimal::from_str_exact(s).unwrap())
}

fn list(items: Vec<InnerValue>) -> InnerValue {
    v_list(items)
}

fn ints(xs: &[i64]) -> InnerValue {
    v_list(xs.iter().map(|&n| InnerValue::Int(n)).collect())
}

fn strs(xs: &[&str]) -> InnerValue {
    v_list(xs.iter().map(|&s| InnerValue::Str(s.into())).collect())
}

#[test]
fn length_ok_and_type_error() {
    let r = reg();
    assert_eq!(r.call("length", &[ints(&[1, 2, 3])]).unwrap(), v_int(3));
    assert_eq!(r.call("length", &[list(vec![])]).unwrap(), v_int(0));
    // error: not a list
    assert_eq!(
        r.call("length", &[InnerValue::Int(7)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn get_ok_and_out_of_range() {
    let r = reg();
    assert_eq!(
        r.call("get", &[ints(&[10, 20, 30]), InnerValue::Int(1)])
            .unwrap(),
        InnerValue::Int(20)
    );
    // error: index past end
    assert_eq!(
        r.call("get", &[ints(&[10]), InnerValue::Int(5)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
    // error: negative index
    assert_eq!(
        r.call("get", &[ints(&[10]), InnerValue::Int(-1)])
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
                InnerValue::Int(1),
                InnerValue::Int(2)
            ]
        )
        .unwrap(),
        ints(&[2, 3])
    );
    // len past end is clamped to the array tail
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), InnerValue::Int(2), InnerValue::Int(99)]
        )
        .unwrap(),
        ints(&[3])
    );
    // start past end -> empty
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), InnerValue::Int(10), InnerValue::Int(2)]
        )
        .unwrap(),
        list(vec![])
    );
    // error: negative length
    assert_eq!(
        r.call(
            "slice",
            &[ints(&[1, 2, 3]), InnerValue::Int(0), InnerValue::Int(-1)]
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
        r.call("contains", &[ints(&[1, 2, 3]), InnerValue::Int(2)])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("contains", &[ints(&[1, 2, 3]), InnerValue::Int(9)])
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
            &[strs(&["a", "b", "c"]), InnerValue::Str("b".into())]
        )
        .unwrap(),
        v_int(1)
    );
    // absent -> -1
    assert_eq!(
        r.call("index_of", &[strs(&["a"]), InnerValue::Str("z".into())])
            .unwrap(),
        v_int(-1)
    );
    // error: first arg not a list
    assert_eq!(
        r.call("index_of", &[InnerValue::Int(1), InnerValue::Int(1)])
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
        InnerValue::Int(7)
    );
    assert_eq!(
        r.call("last", &[ints(&[7, 8, 9])]).unwrap(),
        InnerValue::Int(9)
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
    let bad = list(vec![ints(&[1]), InnerValue::Int(2)]);
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
        r.call("distinct", &[InnerValue::Bool(true)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
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
            &[strs(&["a", "b", "c"]), InnerValue::Str("-".into())]
        )
        .unwrap(),
        v_str("a-b-c".into())
    );
    // edge: empty array -> empty string
    assert_eq!(
        r.call("join", &[list(vec![]), InnerValue::Str(",".into())])
            .unwrap(),
        v_str(String::new())
    );
    // error: non-string element
    assert_eq!(
        r.call("join", &[ints(&[1, 2]), InnerValue::Str(",".into())])
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
        InnerValue::Int(-1)
    );
    assert_eq!(
        r.call("max", &[ints(&[5, 2, 8, -1])]).unwrap(),
        InnerValue::Int(8)
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
        InnerValue::Bool(true),
        InnerValue::Int(5),
        InnerValue::Str("a".into()),
    ]);
    assert_eq!(
        r.call("min", std::slice::from_ref(&mixed)).unwrap(),
        InnerValue::Bool(true)
    );
    assert_eq!(
        r.call("max", std::slice::from_ref(&mixed)).unwrap(),
        InnerValue::Str("a".into())
    );
    // max no longer errors on strings — it uses cross-type compare.
    assert_eq!(
        r.call("max", &[strs(&["x"])]).unwrap(),
        InnerValue::Str("x".into())
    );
}
