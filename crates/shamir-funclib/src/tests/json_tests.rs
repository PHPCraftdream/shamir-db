use crate::json;
use crate::registry::ScalarRegistry;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::TMap;
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    json::register(&mut r);
    r
}

fn map(pairs: Vec<(u64, InnerValue)>) -> InnerValue {
    let mut m: TMap<InternerKey, InnerValue> = TMap::default();
    for (k, v) in pairs {
        m.insert(InternerKey::new(k), v);
    }
    InnerValue::Map(m)
}

fn ints(ns: &[i64]) -> InnerValue {
    InnerValue::List(ns.iter().map(|n| InnerValue::Int(*n)).collect())
}

#[test]
fn get_path_into_list_and_map() {
    let r = reg();
    // root = { 1: [10, 20, 30] }
    let root = map(vec![(1, ints(&[10, 20, 30]))]);
    // path [1, 2] -> map key 1, then list index 2 -> 30
    assert_eq!(
        r.call("get_path", &[root.clone(), ints(&[1, 2])]).unwrap(),
        InnerValue::Int(30)
    );
    // negative index: [1, -1] -> last element -> 30
    assert_eq!(
        r.call("get_path", &[root.clone(), ints(&[1, -1])]).unwrap(),
        InnerValue::Int(30)
    );
    // empty path -> the root itself
    let empty = InnerValue::List(vec![]);
    let root2 = map(vec![(1, ints(&[10, 20, 30]))]);
    assert_eq!(r.call("get_path", &[root, empty]).unwrap(), root2);
}

#[test]
fn get_path_miss_returns_null() {
    let r = reg();
    let root = map(vec![(1, ints(&[10, 20]))]);
    // absent key
    assert_eq!(
        r.call("get_path", &[root.clone(), ints(&[99])]).unwrap(),
        InnerValue::Null
    );
    // out-of-range index
    assert_eq!(
        r.call("get_path", &[root.clone(), ints(&[1, 5])]).unwrap(),
        InnerValue::Null
    );
    // descending into a scalar
    assert_eq!(
        r.call("get_path", &[root, ints(&[1, 0, 0])]).unwrap(),
        InnerValue::Null
    );
}

#[test]
fn get_path_malformed_step_errors() {
    let r = reg();
    let root = ints(&[1, 2]);
    let bad = InnerValue::List(vec![InnerValue::Str("x".into())]);
    assert_eq!(
        r.call("get_path", &[root, bad]).unwrap_err().code,
        "type_mismatch"
    );
    // second arg not a list
    let err = r
        .call("get_path", &[InnerValue::Null, InnerValue::Int(0)])
        .unwrap_err();
    assert_eq!(err.code, "type_mismatch");
}

#[test]
fn array_length_ok_and_err() {
    let r = reg();
    assert_eq!(
        r.call("array_length", &[ints(&[1, 2, 3])]).unwrap(),
        InnerValue::Int(3)
    );
    assert_eq!(
        r.call("array_length", &[InnerValue::List(vec![])]).unwrap(),
        InnerValue::Int(0)
    );
    assert_eq!(
        r.call("array_length", &[InnerValue::Int(1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn keys_ok_and_err() {
    let r = reg();
    let m = map(vec![(1, InnerValue::Int(10)), (7, InnerValue::Int(70))]);
    let got = r.call("keys", &[m]).unwrap();
    assert_eq!(got, ints(&[1, 7]));
    // non-map -> error
    assert_eq!(
        r.call("keys", &[InnerValue::Int(1)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn type_of_variants() {
    let r = reg();
    assert_eq!(
        r.call("type_of", &[InnerValue::Int(1)]).unwrap(),
        InnerValue::Str("int".into())
    );
    assert_eq!(
        r.call("type_of", &[InnerValue::Str("s".into())]).unwrap(),
        InnerValue::Str("string".into())
    );
    assert_eq!(
        r.call("type_of", &[InnerValue::List(vec![])]).unwrap(),
        InnerValue::Str("list".into())
    );
    assert_eq!(
        r.call("type_of", &[map(vec![])]).unwrap(),
        InnerValue::Str("map".into())
    );
    assert_eq!(
        r.call("type_of", &[InnerValue::Bool(true)]).unwrap(),
        InnerValue::Str("bool".into())
    );
    assert_eq!(
        r.call("type_of", &[InnerValue::Null]).unwrap(),
        InnerValue::Str("null".into())
    );
}

#[test]
fn exists_true_false_and_err() {
    let r = reg();
    let root = map(vec![(1, ints(&[10, 20]))]);
    assert_eq!(
        r.call("exists", &[root.clone(), ints(&[1, 0])]).unwrap(),
        InnerValue::Bool(true)
    );
    assert_eq!(
        r.call("exists", &[root.clone(), ints(&[1, 99])]).unwrap(),
        InnerValue::Bool(false)
    );
    // malformed step still errors
    let bad = InnerValue::List(vec![InnerValue::Bool(true)]);
    assert_eq!(
        r.call("exists", &[root, bad]).unwrap_err().code,
        "type_mismatch"
    );
}
