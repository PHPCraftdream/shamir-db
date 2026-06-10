use crate::object;
use crate::registry::ScalarRegistry;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    object::register(&mut r);
    r
}

/// Build a map from `(id, value)` pairs, preserving insertion order.
fn map(pairs: &[(u64, InnerValue)]) -> InnerValue {
    let mut m: TMap<InternerKey, InnerValue> = new_map_wc(pairs.len());
    for (id, v) in pairs {
        m.insert(InternerKey::new(*id), v.clone());
    }
    InnerValue::Map(m)
}

fn sample() -> InnerValue {
    map(&[
        (1, InnerValue::Int(10)),
        (2, InnerValue::Str("hi".into())),
        (3, InnerValue::Bool(true)),
    ])
}

#[test]
fn keys_ok_and_type_error() {
    let r = reg();
    assert_eq!(
        r.call("keys", &[sample()]).unwrap(),
        InnerValue::List(vec![
            InnerValue::Int(1),
            InnerValue::Int(2),
            InnerValue::Int(3),
        ])
    );
    // error: not a map
    assert_eq!(
        r.call("keys", &[InnerValue::Int(7)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn values_ok_and_arity() {
    let r = reg();
    assert_eq!(
        r.call("values", &[sample()]).unwrap(),
        InnerValue::List(vec![
            InnerValue::Int(10),
            InnerValue::Str("hi".into()),
            InnerValue::Bool(true),
        ])
    );
    // error: missing arg -> arity
    assert_eq!(r.call("values", &[]).unwrap_err().code, "arity");
}

#[test]
fn entries_ok() {
    let r = reg();
    assert_eq!(
        r.call("entries", &[map(&[(5, InnerValue::Int(99))])])
            .unwrap(),
        InnerValue::List(vec![InnerValue::List(vec![
            InnerValue::Int(5),
            InnerValue::Int(99),
        ])])
    );
    // edge: empty map -> empty list
    assert_eq!(
        r.call("entries", &[map(&[])]).unwrap(),
        InnerValue::List(vec![])
    );
}

#[test]
fn has_key_true_false_and_bad_key() {
    let r = reg();
    assert_eq!(
        r.call("has_key", &[sample(), InnerValue::Int(2)]).unwrap(),
        InnerValue::Bool(true)
    );
    assert_eq!(
        r.call("has_key", &[sample(), InnerValue::Int(99)]).unwrap(),
        InnerValue::Bool(false)
    );
    // error: negative key id
    assert_eq!(
        r.call("has_key", &[sample(), InnerValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn get_path_nested_and_missing() {
    let r = reg();
    let nested = map(&[(1, map(&[(2, map(&[(3, InnerValue::Str("deep".into()))]))]))]);
    assert_eq!(
        r.call(
            "get_path",
            &[
                nested.clone(),
                InnerValue::List(vec![
                    InnerValue::Int(1),
                    InnerValue::Int(2),
                    InnerValue::Int(3),
                ]),
            ],
        )
        .unwrap(),
        InnerValue::Str("deep".into())
    );
    // missing intermediate key
    assert_eq!(
        r.call(
            "get_path",
            &[
                nested,
                InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(9)]),
            ],
        )
        .unwrap_err()
        .code,
        "missing_key"
    );
    // head not a map
    assert_eq!(
        r.call("get_path", &[InnerValue::Int(1), InnerValue::List(vec![])],)
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn merge_right_wins() {
    let r = reg();
    let a = map(&[(1, InnerValue::Int(1)), (2, InnerValue::Int(2))]);
    let b = map(&[(2, InnerValue::Int(20)), (3, InnerValue::Int(30))]);
    assert_eq!(
        r.call("merge", &[a, b]).unwrap(),
        map(&[
            (1, InnerValue::Int(1)),
            (2, InnerValue::Int(20)),
            (3, InnerValue::Int(30)),
        ])
    );
    // error: second arg not a map
    assert_eq!(
        r.call("merge", &[map(&[]), InnerValue::Int(0)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn pick_keeps_only_selected() {
    let r = reg();
    assert_eq!(
        r.call(
            "pick",
            &[
                sample(),
                InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(3)])
            ],
        )
        .unwrap(),
        map(&[(1, InnerValue::Int(10)), (3, InnerValue::Bool(true))])
    );
    // missing keys are silently skipped
    assert_eq!(
        r.call(
            "pick",
            &[sample(), InnerValue::List(vec![InnerValue::Int(99)])],
        )
        .unwrap(),
        map(&[])
    );
    // error: non-int key in list
    assert_eq!(
        r.call(
            "pick",
            &[
                sample(),
                InnerValue::List(vec![InnerValue::Str("x".into())])
            ],
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
        r.call(
            "omit",
            &[sample(), InnerValue::List(vec![InnerValue::Int(2)])],
        )
        .unwrap(),
        map(&[(1, InnerValue::Int(10)), (3, InnerValue::Bool(true))])
    );
    // omitting a non-present key is a no-op
    assert_eq!(
        r.call(
            "omit",
            &[sample(), InnerValue::List(vec![InnerValue::Int(99)])],
        )
        .unwrap(),
        sample()
    );
}
