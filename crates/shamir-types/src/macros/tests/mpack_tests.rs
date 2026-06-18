use crate::mpack;
use crate::types::common::new_map;
use crate::types::value::QueryValue;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use std::str::FromStr;

// -----------------------------------------------------------------------
// Scalar variants — each compared against a manually-constructed QueryValue
// -----------------------------------------------------------------------

#[test]
fn mpack_null() {
    assert_eq!(mpack!(null), QueryValue::Null);
}

#[test]
fn mpack_bool_true() {
    assert_eq!(mpack!(true), QueryValue::Bool(true));
}

#[test]
fn mpack_bool_false() {
    assert_eq!(mpack!(false), QueryValue::Bool(false));
}

#[test]
fn mpack_int_positive() {
    assert_eq!(mpack!(42), QueryValue::Int(42));
}

#[test]
fn mpack_int_negative() {
    assert_eq!(mpack!(-7), QueryValue::Int(-7));
}

#[test]
fn mpack_int_zero() {
    assert_eq!(mpack!(0), QueryValue::Int(0));
}

#[test]
fn mpack_int_large() {
    assert_eq!(
        mpack!(9_223_372_036_854_775_807i64),
        QueryValue::Int(i64::MAX)
    );
}

#[test]
fn mpack_float() {
    assert_eq!(mpack!(3.5), QueryValue::F64(3.5));
}

#[test]
fn mpack_float_negative() {
    assert_eq!(mpack!(-2.5), QueryValue::F64(-2.5));
}

#[test]
fn mpack_float_zero() {
    assert_eq!(mpack!(0.0), QueryValue::F64(0.0));
}

#[test]
fn mpack_float_explicit_suffix() {
    // Explicit suffix forces F64 even for a whole number value.
    assert_eq!(mpack!(1.0f64), QueryValue::F64(1.0));
}

#[test]
fn mpack_str() {
    assert_eq!(mpack!("hello"), QueryValue::Str("hello".to_string()));
}

#[test]
fn mpack_str_empty() {
    assert_eq!(mpack!(""), QueryValue::Str(String::new()));
}

#[test]
fn mpack_str_unicode() {
    assert_eq!(
        mpack!("Привет 🌍"),
        QueryValue::Str("Привет 🌍".to_string())
    );
}

// -----------------------------------------------------------------------
// List variants
// -----------------------------------------------------------------------

#[test]
fn mpack_empty_list() {
    assert_eq!(mpack!([]), QueryValue::List(vec![]));
}

#[test]
fn mpack_simple_list() {
    let expected = QueryValue::List(vec![
        QueryValue::Int(1),
        QueryValue::Int(2),
        QueryValue::Int(3),
    ]);
    assert_eq!(mpack!([1, 2, 3]), expected);
}

#[test]
fn mpack_list_trailing_comma() {
    let expected = QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]);
    assert_eq!(mpack!([1, 2,]), expected);
}

#[test]
fn mpack_list_mixed_types() {
    let expected = QueryValue::List(vec![
        QueryValue::Null,
        QueryValue::Bool(true),
        QueryValue::Int(99),
        QueryValue::F64(0.5),
        QueryValue::Str("x".to_string()),
    ]);
    assert_eq!(mpack!([null, true, 99, 0.5, "x"]), expected);
}

#[test]
fn mpack_nested_list() {
    let expected = QueryValue::List(vec![
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(2)]),
        QueryValue::List(vec![QueryValue::Int(3), QueryValue::Int(4)]),
    ]);
    assert_eq!(mpack!([[1, 2], [3, 4]]), expected);
}

// -----------------------------------------------------------------------
// Map variants
// -----------------------------------------------------------------------

#[test]
fn mpack_empty_map() {
    assert_eq!(mpack!({}), QueryValue::Map(new_map()));
}

#[test]
fn mpack_simple_map() {
    let mut expected = new_map();
    expected.insert("a".to_string(), QueryValue::Int(1));
    expected.insert("b".to_string(), QueryValue::Bool(true));
    assert_eq!(mpack!({ "a": 1, "b": true }), QueryValue::Map(expected));
}

#[test]
fn mpack_map_trailing_comma() {
    let mut expected = new_map();
    expected.insert("x".to_string(), QueryValue::Int(42));
    assert_eq!(mpack!({ "x": 42, }), QueryValue::Map(expected));
}

#[test]
fn mpack_map_string_value() {
    let mut expected = new_map();
    expected.insert("name".to_string(), QueryValue::Str("Alice".to_string()));
    assert_eq!(mpack!({ "name": "Alice" }), QueryValue::Map(expected));
}

#[test]
fn mpack_map_null_value() {
    let mut expected = new_map();
    expected.insert("deleted".to_string(), QueryValue::Null);
    assert_eq!(mpack!({ "deleted": null }), QueryValue::Map(expected));
}

#[test]
fn mpack_nested_map() {
    let mut inner = new_map();
    inner.insert("y".to_string(), QueryValue::Int(2));
    let mut outer = new_map();
    outer.insert("x".to_string(), QueryValue::Map(inner));

    assert_eq!(mpack!({ "x": { "y": 2 } }), QueryValue::Map(outer));
}

#[test]
fn mpack_map_with_list_value() {
    let mut expected = new_map();
    expected.insert(
        "items".to_string(),
        QueryValue::List(vec![QueryValue::Int(10), QueryValue::Int(20)]),
    );
    assert_eq!(mpack!({ "items": [10, 20] }), QueryValue::Map(expected));
}

// -----------------------------------------------------------------------
// Escape hatch `@` — Dec, Big, Bin, runtime variables
// -----------------------------------------------------------------------

#[test]
fn mpack_escape_decimal() {
    let d = Decimal::from_str("123.456").unwrap();
    let expected = QueryValue::Dec(d);
    assert_eq!(mpack!(@ QueryValue::Dec(d)), expected);
}

#[test]
fn mpack_escape_bigint() {
    let b = BigInt::from_str("999999999999999999999999999999").unwrap();
    let expected = QueryValue::Big(b.clone());
    assert_eq!(mpack!(@ QueryValue::Big(b)), expected);
}

#[test]
fn mpack_escape_bin() {
    let bytes = vec![0xCA, 0xFE, 0xBA, 0xBE];
    let expected = QueryValue::Bin(bytes.clone());
    assert_eq!(mpack!(@ QueryValue::Bin(bytes)), expected);
}

#[test]
fn mpack_escape_in_map_value() {
    let d = Decimal::from_str("10.99").unwrap();
    let mut expected = new_map();
    expected.insert("price".to_string(), QueryValue::Dec(d));
    expected.insert("qty".to_string(), QueryValue::Int(5));

    let result = mpack!({ "price": @(QueryValue::Dec(d)), "qty": 5 });
    assert_eq!(result, QueryValue::Map(expected));
}

#[test]
fn mpack_escape_variable() {
    let val = QueryValue::Str("dynamic".to_string());
    assert_eq!(mpack!(@val), QueryValue::Str("dynamic".to_string()));
}

#[test]
fn mpack_escape_in_list() {
    let b = BigInt::from(u128::MAX);
    let expected = QueryValue::List(vec![QueryValue::Int(1), QueryValue::Big(b.clone())]);
    assert_eq!(mpack!([1, @(QueryValue::Big(b))]), expected);
}

// -----------------------------------------------------------------------
// Deep nesting: map → list → map
// -----------------------------------------------------------------------

#[test]
fn mpack_deep_nesting() {
    // Build the expected value by hand.
    let mut inner_map = new_map();
    inner_map.insert("z".to_string(), QueryValue::Int(99));

    let list = QueryValue::List(vec![QueryValue::Int(1), QueryValue::Map(inner_map)]);

    let mut outer = new_map();
    outer.insert("data".to_string(), list);

    let expected = QueryValue::Map(outer);

    let result = mpack!({
        "data": [1, { "z": 99 }]
    });
    assert_eq!(result, expected);
}

#[test]
fn mpack_deeply_nested_maps() {
    // Three levels deep.
    let mut lvl2 = new_map();
    lvl2.insert("leaf".to_string(), QueryValue::Bool(false));

    let mut lvl1 = new_map();
    lvl1.insert("mid".to_string(), QueryValue::Map(lvl2));

    let mut root = new_map();
    root.insert("top".to_string(), QueryValue::Map(lvl1));

    let result = mpack!({ "top": { "mid": { "leaf": false } } });
    assert_eq!(result, QueryValue::Map(root));
}

// -----------------------------------------------------------------------
// Result type — every call produces exactly QueryValue
// -----------------------------------------------------------------------

#[test]
fn mpack_result_type_is_query_value() {
    // The binding annotation proves the type statically.
    let _: QueryValue = mpack!(null);
    let _: QueryValue = mpack!(true);
    let _: QueryValue = mpack!(42);
    let _: QueryValue = mpack!(3.5);
    let _: QueryValue = mpack!("s");
    let _: QueryValue = mpack!([]);
    let _: QueryValue = mpack!({});
}

// -----------------------------------------------------------------------
// Edge cases
// -----------------------------------------------------------------------

#[test]
fn mpack_single_element_list() {
    assert_eq!(mpack!([null]), QueryValue::List(vec![QueryValue::Null]));
}

#[test]
fn mpack_map_multiple_fields() {
    let mut expected = new_map();
    expected.insert("a".to_string(), QueryValue::Int(1));
    expected.insert("b".to_string(), QueryValue::Int(2));
    expected.insert("c".to_string(), QueryValue::Int(3));
    expected.insert("d".to_string(), QueryValue::Int(4));

    let result = mpack!({ "a": 1, "b": 2, "c": 3, "d": 4 });
    assert_eq!(result, QueryValue::Map(expected));
}

#[test]
fn mpack_list_of_maps() {
    let mut m1 = new_map();
    m1.insert("id".to_string(), QueryValue::Int(1));
    let mut m2 = new_map();
    m2.insert("id".to_string(), QueryValue::Int(2));

    let expected = QueryValue::List(vec![QueryValue::Map(m1), QueryValue::Map(m2)]);
    let result = mpack!([{ "id": 1 }, { "id": 2 }]);
    assert_eq!(result, expected);
}

#[test]
fn mpack_escape_set() {
    use crate::types::common::new_set;
    let mut s = new_set();
    s.insert(QueryValue::Int(1));
    s.insert(QueryValue::Int(2));
    let expected = QueryValue::Set(s.clone());
    assert_eq!(mpack!(@ QueryValue::Set(s)), expected);
}
