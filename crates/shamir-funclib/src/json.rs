//! `/json` scalar category — structural navigation over the [`InnerValue`]
//! `Map` / `List` tree, with no external dependency.
//!
//! Functions registered (plain names, no folder prefix):
//! `get_path array_length keys type_of exists`.
//!
//! Conventions:
//! - A *path* is a `List` of step values: an `Int` selects a `List` index
//!   (negative indices count from the end) **or** a `Map` key by its interned
//!   `u64` id (map keys are interned numeric ids, not strings, at this layer).
//! - [`get_path`] returns `Null` for any miss (out-of-range index, absent key,
//!   or descending into a scalar); it never errors on a structurally valid
//!   path — only a malformed path (non-`Int` step) yields `"type_mismatch"`.
//! - [`exists`] is the boolean companion of [`get_path`].
//! - [`type_of`] returns a `Str` naming the variant; [`keys`] returns the
//!   `Map`'s key ids as an `Int` `List`; [`array_length`] returns an `Int`.
//! - All functions are pure + deterministic.

use crate::registry::{
    arg_list, v_bool, v_int, v_list, v_str, FnEntry, ScalarError, ScalarRegistry,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

/// Register the `/json` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "get_path",
        FnEntry::pure(
            |a| {
                let steps = arg_list(a, 1)?;
                Ok(navigate(&a[0], steps)?.cloned().unwrap_or(InnerValue::Null))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "array_length",
        FnEntry::pure(
            |a| match &a[0] {
                InnerValue::List(l) => Ok(v_int(l.len() as i64)),
                _ => Err(ScalarError::new("type_mismatch")),
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "keys",
        FnEntry::pure(
            |a| match &a[0] {
                InnerValue::Map(m) => Ok(v_list(m.keys().map(|k| v_int(k.id() as i64)).collect())),
                _ => Err(ScalarError::new("type_mismatch")),
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "type_of",
        FnEntry::pure(|a| Ok(v_str(type_name(&a[0]).to_string())), 1, Some(1)),
    );
    reg.register(
        "exists",
        FnEntry::pure(
            |a| {
                let steps = arg_list(a, 1)?;
                Ok(v_bool(navigate(&a[0], steps)?.is_some()))
            },
            2,
            Some(2),
        ),
    );
}

/// The stable variant name returned by `type_of`.
fn type_name(v: &InnerValue) -> &'static str {
    match v {
        InnerValue::Null => "null",
        InnerValue::Bool(_) => "bool",
        InnerValue::Int(_) => "int",
        InnerValue::F64(_) => "f64",
        InnerValue::Dec(_) => "dec",
        InnerValue::Big(_) => "big",
        InnerValue::Str(_) => "string",
        InnerValue::Bin(_) => "bytes",
        InnerValue::List(_) => "list",
        InnerValue::Set(_) => "set",
        InnerValue::Map(_) => "map",
    }
}

/// Walk `root` following `steps`. Each step is an `Int`: a `List` index
/// (negative counts from the end) or a `Map` key id. Returns `Ok(None)` for a
/// structural miss, `Ok(Some(&value))` on success, and `"type_mismatch"` only
/// when a step is not an `Int`.
fn navigate<'a>(
    root: &'a InnerValue,
    steps: &[InnerValue],
) -> Result<Option<&'a InnerValue>, ScalarError> {
    let mut cur = root;
    for step in steps {
        let idx = match step {
            InnerValue::Int(n) => *n,
            _ => return Err(ScalarError::new("type_mismatch")),
        };
        match cur {
            InnerValue::List(l) => {
                let len = l.len() as i64;
                let resolved = if idx < 0 { len + idx } else { idx };
                if resolved < 0 || resolved >= len {
                    return Ok(None);
                }
                cur = &l[resolved as usize];
            }
            InnerValue::Map(m) => {
                if idx < 0 {
                    return Ok(None);
                }
                match m.get(&InternerKey::new(idx as u64)) {
                    Some(v) => cur = v,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(cur))
}

#[cfg(test)]
mod tests {
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
}
