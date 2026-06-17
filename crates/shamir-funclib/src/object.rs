//! `/object` scalar category — map (object) introspection and reshaping.
//!
//! Functions registered (plain names, no folder prefix):
//! `keys values entries has_key get_path merge pick omit`.
//!
//! Value-model notes (mirrors `math.rs` conventions):
//! - An "object" is a [`QueryValue::Map`] keyed by `String` field names.
//! - `keys` / `values` return a `List`; `entries` returns a `List` of two-item
//!   `[name, value]` `List`s, with each name as a `Str`.
//! - `has_key` returns a `Bool`; `get_path` walks nested maps and yields
//!   `ScalarError("missing_key")` if any step is absent or a non-map is
//!   traversed.
//! - `merge` / `pick` / `omit` return a fresh `Map`, preserving insertion order
//!   (the left/source order); `merge` lets the right-hand value win on key
//!   collisions.
//! - Every function here is pure + deterministic.

use crate::registry::{
    arg_list, arg_str, v_bool, v_list, v_str, FnEntry, ScalarError, ScalarRegistry,
};
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::QueryValue;

/// Extract a `&TMap<String, QueryValue>` from the `i`-th argument or
/// `ScalarError("type_mismatch")`.
fn arg_map(args: &[QueryValue], i: usize) -> Result<&TMap<String, QueryValue>, ScalarError> {
    match args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))? {
        QueryValue::Map(m) => Ok(m),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Read the `i`-th argument as a string key.
fn arg_key(args: &[QueryValue], i: usize) -> Result<String, ScalarError> {
    Ok(arg_str(args, i)?.to_owned())
}

/// Construct a `Map`.
fn v_map(m: TMap<String, QueryValue>) -> QueryValue {
    QueryValue::Map(m)
}

/// Register the `/object` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "keys",
        FnEntry::pure(
            |a| {
                let m = arg_map(a, 0)?;
                Ok(v_list(m.keys().map(|k| v_str(k.clone())).collect()))
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
                        .map(|(k, v)| v_list(vec![v_str(k.clone()), v.clone()]))
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
                        QueryValue::Map(m) => m,
                        _ => return Err(ScalarError::new("missing_key")),
                    };
                    let key = match step {
                        QueryValue::Str(s) => s.as_str(),
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    cur = m
                        .get(key)
                        .ok_or_else(|| ScalarError::new("missing_key"))?;
                }
                // The head must itself be a map (validated only if a step ran;
                // validate explicitly so a 0-length path still type-checks).
                if !matches!(a[0], QueryValue::Map(_)) {
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
                    let key = match step {
                        QueryValue::Str(s) => s.as_str(),
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    if let Some(v) = m.get(key) {
                        out.insert(key.to_owned(), v.clone());
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
                let mut drop = TMap::<String, ()>::default();
                for step in keys {
                    let key = match step {
                        QueryValue::Str(s) => s.clone(),
                        _ => return Err(ScalarError::new("type_mismatch")),
                    };
                    drop.insert(key, ());
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
