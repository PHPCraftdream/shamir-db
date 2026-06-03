//! `/object` scalar category — map (object) introspection and reshaping.
//!
//! Functions registered (plain names, no folder prefix):
//! `keys values entries has_key get_path merge pick omit`.
//!
//! Value-model notes (mirrors `math.rs` conventions):
//! - An "object" is an [`InnerValue::Map`] keyed by [`InternerKey`] (interned
//!   `u64` field ids). Keys are therefore addressed by their integer id: every
//!   `key` / path element argument is read via [`arg_i64`] and must be a
//!   non-negative `u64` ([`ScalarError`]`("out_of_range")` otherwise).
//! - `keys` / `values` return a `List`; `entries` returns a `List` of two-item
//!   `[id, value]` `List`s, with each id as an `Int`.
//! - `has_key` returns a `Bool`; `get_path` walks nested maps and yields
//!   `ScalarError("missing_key")` if any step is absent or a non-map is
//!   traversed.
//! - `merge` / `pick` / `omit` return a fresh `Map`, preserving insertion order
//!   (the left/source order); `merge` lets the right-hand value win on key
//!   collisions.
//! - Every function here is pure + deterministic.

use crate::registry::{
    arg_i64, arg_list, v_bool, v_int, v_list, FnEntry, ScalarError, ScalarRegistry,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::InnerValue;

/// Extract a `&TMap` from the `i`-th argument or `ScalarError("type_mismatch")`.
fn arg_map(args: &[InnerValue], i: usize) -> Result<&TMap<InternerKey, InnerValue>, ScalarError> {
    match args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))? {
        InnerValue::Map(m) => Ok(m),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Read the `i`-th argument as a non-negative field id ([`InternerKey`]).
fn arg_key(args: &[InnerValue], i: usize) -> Result<InternerKey, ScalarError> {
    let id = arg_i64(args, i)?;
    let id = u64::try_from(id).map_err(|_| ScalarError::new("out_of_range"))?;
    Ok(InternerKey::new(id))
}

/// Construct a `Map`.
fn v_map(m: TMap<InternerKey, InnerValue>) -> InnerValue {
    InnerValue::Map(m)
}

/// Register the `/object` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "keys",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                Ok(v_list(m.keys().map(|k| v_int(k.id() as i64)).collect()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "values",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                Ok(v_list(m.values().cloned().collect()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "entries",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                Ok(v_list(
                    m.iter()
                        .map(|(k, v)| v_list(vec![v_int(k.id() as i64), v.clone()]))
                        .collect(),
                ))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "has_key",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                let k = arg_key(a, 1)?;
                Ok(v_bool(m.contains_key(&k)))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "get_path",
        FnEntry::pure(
            |a| {
                let mut cur = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                let path = arg_list(a, 1)?;
                for step in path {
                    let m = match cur {
                        InnerValue::Map(m) => m,
                        _ => return Err(ScalarError::new("missing_key")),
                    };
                    let id = match step {
                        InnerValue::Int(n) => {
                            u64::try_from(*n).map_err(|_| ScalarError::new("out_of_range"))?
                        }
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    cur = m
                        .get(&InternerKey::new(id))
                        .ok_or_else(|| ScalarError::new("missing_key"))?;
                }
                // The head must itself be a map (validated only if a step ran;
                // validate explicitly so a 0-length path still type-checks).
                if !matches!(a[0], InnerValue::Map(_)) {
                    return Err(ScalarError::new("type_mismatch"));
                }
                Ok(cur.clone())
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "merge",
        FnEntry::pure(
            |a| {
                let left = arg_map(a, 0)?;
                let right = arg_map(a, 1)?;
                let mut out = new_map_wc(left.len() + right.len());
                for (k, v) in left.iter() {
                    out.insert(k.clone(), v.clone());
                }
                for (k, v) in right.iter() {
                    out.insert(k.clone(), v.clone());
                }
                Ok(v_map(out))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "pick",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                let keys = arg_list(a, 1)?;
                let mut out = new_map_wc(keys.len());
                for step in keys {
                    let id = match step {
                        InnerValue::Int(n) => {
                            u64::try_from(*n).map_err(|_| ScalarError::new("out_of_range"))?
                        }
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    let key = InternerKey::new(id);
                    if let Some(v) = m.get(&key) {
                        out.insert(key, v.clone());
                    }
                }
                Ok(v_map(out))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "omit",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                let keys = arg_list(a, 1)?;
                let mut drop = TMap::<InternerKey, ()>::default();
                for step in keys {
                    let id = match step {
                        InnerValue::Int(n) => {
                            u64::try_from(*n).map_err(|_| ScalarError::new("out_of_range"))?
                        }
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    drop.insert(InternerKey::new(id), ());
                }
                let mut out = new_map_wc(m.len());
                for (k, v) in m.iter() {
                    if !drop.contains_key(k) {
                        out.insert(k.clone(), v.clone());
                    }
                }
                Ok(v_map(out))
            },
            2,
            Some(2),
        ),
    );
}

#[cfg(test)]
mod tests {
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
}
