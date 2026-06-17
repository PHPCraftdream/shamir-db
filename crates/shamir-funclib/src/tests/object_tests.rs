use crate::object;
use crate::registry::ScalarRegistry;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    object::register(&mut r);
    r
}

/// Build a string-keyed map from `(name, value)` pairs, preserving insertion order.
fn map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m: TMap<String, QueryValue> = new_map_wc(pairs.len());
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    QueryValue::Map(m)
}

fn sample() -> QueryValue {
    map(&[
        ("a", QueryValue::Int(10)),
        ("b", QueryValue::Str("hi".into())),
        ("c", QueryValue::Bool(true)),
    ])
}

fn s(v: &str) -> QueryValue {
    QueryValue::Str(v.to_owned())
}

#[test]
fn keys_ok_and_type_error() {
    let r = reg();
    // NOTE: Under QueryValue ABI, keys returns string names.
    assert_eq!(
        r.call("keys", &[sample()]).unwrap(),
        QueryValue::List(vec![s("a"), s("b"), s("c"),])
    );
    // error: not a map
    assert_eq!(
        r.call("keys", &[QueryValue::Int(7)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn values_ok_and_arity() {
    let r = reg();
    assert_eq!(
        r.call("values", &[sample()]).unwrap(),
        QueryValue::List(vec![
            QueryValue::Int(10),
            QueryValue::Str("hi".into()),
            QueryValue::Bool(true),
        ])
    );
    // error: missing arg -> arity
    assert_eq!(r.call("values", &[]).unwrap_err().code, "arity");
}

#[test]
fn entries_ok() {
    let r = reg();
    // NOTE: Under QueryValue ABI, entries returns [name_str, value] pairs.
    assert_eq!(
        r.call("entries", &[map(&[("x", QueryValue::Int(99))])])
            .unwrap(),
        QueryValue::List(vec![QueryValue::List(vec![s("x"), QueryValue::Int(99),])])
    );
    // edge: empty map -> empty list
    assert_eq!(
        r.call("entries", &[map(&[])]).unwrap(),
        QueryValue::List(vec![])
    );
}

#[test]
fn has_key_true_false_and_bad_key() {
    let r = reg();
    // has_key now takes a string key
    assert_eq!(
        r.call("has_key", &[sample(), s("b")]).unwrap(),
        QueryValue::Bool(true)
    );
    assert_eq!(
        r.call("has_key", &[sample(), s("zz")]).unwrap(),
        QueryValue::Bool(false)
    );
    // error: non-string key
    assert_eq!(
        r.call("has_key", &[sample(), QueryValue::Int(-1)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn get_path_nested_and_missing() {
    let r = reg();
    let nested = map(&[(
        "x",
        map(&[("y", map(&[("z", QueryValue::Str("deep".into()))]))]),
    )]);
    assert_eq!(
        r.call(
            "get_path",
            &[
                nested.clone(),
                QueryValue::List(vec![s("x"), s("y"), s("z"),]),
            ],
        )
        .unwrap(),
        QueryValue::Str("deep".into())
    );
    // missing intermediate key
    assert_eq!(
        r.call(
            "get_path",
            &[nested, QueryValue::List(vec![s("x"), s("missing")]),],
        )
        .unwrap_err()
        .code,
        "missing_key"
    );
    // head not a map
    assert_eq!(
        r.call("get_path", &[QueryValue::Int(1), QueryValue::List(vec![])],)
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn merge_right_wins() {
    let r = reg();
    let a = map(&[("x", QueryValue::Int(1)), ("y", QueryValue::Int(2))]);
    let b = map(&[("y", QueryValue::Int(20)), ("z", QueryValue::Int(30))]);
    assert_eq!(
        r.call("merge", &[a, b]).unwrap(),
        map(&[
            ("x", QueryValue::Int(1)),
            ("y", QueryValue::Int(20)),
            ("z", QueryValue::Int(30)),
        ])
    );
    // error: second arg not a map
    assert_eq!(
        r.call("merge", &[map(&[]), QueryValue::Int(0)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn pick_keeps_only_selected() {
    let r = reg();
    assert_eq!(
        r.call("pick", &[sample(), QueryValue::List(vec![s("a"), s("c")])],)
            .unwrap(),
        map(&[("a", QueryValue::Int(10)), ("c", QueryValue::Bool(true))])
    );
    // missing keys are silently skipped
    assert_eq!(
        r.call("pick", &[sample(), QueryValue::List(vec![s("zz")])],)
            .unwrap(),
        map(&[])
    );
    // error: non-string key in list
    assert_eq!(
        r.call(
            "pick",
            &[sample(), QueryValue::List(vec![QueryValue::Int(1)])],
        )
        .unwrap_err()
        .code,
        "type_mismatch"
    );
}

#[test]
fn omit_drops_selected() {
    let r = reg();
    assert_eq!(
        r.call("omit", &[sample(), QueryValue::List(vec![s("b")])],)
            .unwrap(),
        map(&[("a", QueryValue::Int(10)), ("c", QueryValue::Bool(true))])
    );
    // omitting a non-present key is a no-op
    assert_eq!(
        r.call("omit", &[sample(), QueryValue::List(vec![s("zz")])],)
            .unwrap(),
        sample()
    );
}
