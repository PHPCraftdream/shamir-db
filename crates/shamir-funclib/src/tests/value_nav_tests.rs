use crate::registry::ScalarRegistry;
use crate::value_nav;
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    value_nav::register(&mut r);
    r
}

fn map(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m: TMap<String, QueryValue> = TMap::default();
    for (k, v) in pairs {
        m.insert(k.to_owned(), v);
    }
    QueryValue::Map(m)
}

fn ints(ns: &[i64]) -> QueryValue {
    QueryValue::List(ns.iter().map(|n| QueryValue::Int(*n)).collect())
}

/// Build a path list from string keys (for Map navigation under QueryValue ABI).
fn str_path(keys: &[&str]) -> QueryValue {
    QueryValue::List(
        keys.iter()
            .map(|k| QueryValue::Str((*k).to_owned()))
            .collect(),
    )
}

#[test]
fn get_path_into_list_and_map() {
    let r = reg();
    // root = { "a": [10, 20, 30] }
    let root = map(vec![("a", ints(&[10, 20, 30]))]);
    // path ["a", 2] -> map key "a", then list index 2 -> 30
    let path = QueryValue::List(vec![QueryValue::Str("a".to_owned()), QueryValue::Int(2)]);
    assert_eq!(
        r.call("get_path", &[root.clone(), path]).unwrap(),
        QueryValue::Int(30)
    );
    // negative index: ["a", -1] -> last element -> 30
    let path_neg = QueryValue::List(vec![QueryValue::Str("a".to_owned()), QueryValue::Int(-1)]);
    assert_eq!(
        r.call("get_path", &[root.clone(), path_neg]).unwrap(),
        QueryValue::Int(30)
    );
    // empty path -> the root itself
    let empty = QueryValue::List(vec![]);
    let root2 = map(vec![("a", ints(&[10, 20, 30]))]);
    assert_eq!(r.call("get_path", &[root, empty]).unwrap(), root2);
}

#[test]
fn get_path_miss_returns_null() {
    let r = reg();
    let root = map(vec![("a", ints(&[10, 20]))]);
    // absent key
    assert_eq!(
        r.call("get_path", &[root.clone(), str_path(&["zz"])])
            .unwrap(),
        QueryValue::Null
    );
    // out-of-range index
    let path_oob = QueryValue::List(vec![QueryValue::Str("a".to_owned()), QueryValue::Int(5)]);
    assert_eq!(
        r.call("get_path", &[root.clone(), path_oob]).unwrap(),
        QueryValue::Null
    );
    // descending into a scalar
    let path_deep = QueryValue::List(vec![
        QueryValue::Str("a".to_owned()),
        QueryValue::Int(0),
        QueryValue::Int(0),
    ]);
    assert_eq!(
        r.call("get_path", &[root, path_deep]).unwrap(),
        QueryValue::Null
    );
}

#[test]
fn get_path_malformed_step_errors() {
    let r = reg();
    let root = ints(&[1, 2]);
    let bad = QueryValue::List(vec![QueryValue::Bool(true)]);
    assert_eq!(
        r.call("get_path", &[root, bad]).unwrap_err().code,
        "type_mismatch"
    );
    // second arg not a list
    let err = r
        .call("get_path", &[QueryValue::Null, QueryValue::Int(0)])
        .unwrap_err();
    assert_eq!(err.code, "type_mismatch");
}

#[test]
fn array_length_ok_and_err() {
    let r = reg();
    assert_eq!(
        r.call("array_length", &[ints(&[1, 2, 3])]).unwrap(),
        QueryValue::Int(3)
    );
    assert_eq!(
        r.call("array_length", &[QueryValue::List(vec![])]).unwrap(),
        QueryValue::Int(0)
    );
    assert_eq!(
        r.call("array_length", &[QueryValue::Int(1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn keys_ok_and_err() {
    let r = reg();
    let m = map(vec![
        ("alpha", QueryValue::Int(10)),
        ("beta", QueryValue::Int(70)),
    ]);
    let got = r.call("keys", &[m]).unwrap();
    // NOTE: Under QueryValue ABI, keys returns string names instead of integer ids.
    assert_eq!(
        got,
        QueryValue::List(vec![
            QueryValue::Str("alpha".to_owned()),
            QueryValue::Str("beta".to_owned()),
        ])
    );
    // non-map -> error
    assert_eq!(
        r.call("keys", &[QueryValue::Int(1)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn type_of_variants() {
    let r = reg();
    assert_eq!(
        r.call("type_of", &[QueryValue::Int(1)]).unwrap(),
        QueryValue::Str("int".into())
    );
    assert_eq!(
        r.call("type_of", &[QueryValue::Str("s".into())]).unwrap(),
        QueryValue::Str("string".into())
    );
    assert_eq!(
        r.call("type_of", &[QueryValue::List(vec![])]).unwrap(),
        QueryValue::Str("list".into())
    );
    assert_eq!(
        r.call("type_of", &[map(vec![])]).unwrap(),
        QueryValue::Str("map".into())
    );
    assert_eq!(
        r.call("type_of", &[QueryValue::Bool(true)]).unwrap(),
        QueryValue::Str("bool".into())
    );
    assert_eq!(
        r.call("type_of", &[QueryValue::Null]).unwrap(),
        QueryValue::Str("null".into())
    );
}

#[test]
fn exists_true_false_and_err() {
    let r = reg();
    let root = map(vec![("a", ints(&[10, 20]))]);
    let path_ok = QueryValue::List(vec![QueryValue::Str("a".to_owned()), QueryValue::Int(0)]);
    assert_eq!(
        r.call("exists", &[root.clone(), path_ok]).unwrap(),
        QueryValue::Bool(true)
    );
    let path_miss = QueryValue::List(vec![QueryValue::Str("a".to_owned()), QueryValue::Int(99)]);
    assert_eq!(
        r.call("exists", &[root.clone(), path_miss]).unwrap(),
        QueryValue::Bool(false)
    );
    // malformed step still errors
    let bad = QueryValue::List(vec![QueryValue::Bool(true)]);
    assert_eq!(
        r.call("exists", &[root, bad]).unwrap_err().code,
        "type_mismatch"
    );
}
