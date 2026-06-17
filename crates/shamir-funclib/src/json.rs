//! `/json` scalar category — structural navigation over the [`QueryValue`]
//! `Map` / `List` tree, with no external dependency.
//!
//! Functions registered (plain names, no folder prefix):
//! `get_path array_length keys type_of exists`.
//!
//! Conventions:
//! - A *path* is a `List` of step values: an `Int` selects a `List` index
//!   (negative indices count from the end) **or** a `Str` selects a `Map` key
//!   by its string name. Under the `QueryValue` ABI, map keys are strings.
//! - [`get_path`] returns `Null` for any miss (out-of-range index, absent key,
//!   or descending into a scalar); it never errors on a structurally valid
//!   path — only a malformed path (non-`Int`/non-`Str` step) yields
//!   `"type_mismatch"`.
//! - [`exists`] is the boolean companion of [`get_path`].
//! - [`type_of`] returns a `Str` naming the variant; [`keys`] returns the
//!   `Map`'s key names as a `Str` `List`; [`array_length`] returns an `Int`.
//! - All functions are pure + deterministic.

use crate::registry::{
    arg_list, v_bool, v_int, v_list, v_str, FnEntry, ScalarError, ScalarRegistry,
};
use shamir_types::types::value::QueryValue;

/// Register the `/json` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "get_path",
        FnEntry::pure(
            |a| {
                let steps = arg_list(a, 1)?;
                Ok(navigate(&a[0], steps)?.cloned().unwrap_or(QueryValue::Null))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "array_length",
        FnEntry::pure(
            |a| match &a[0] {
                QueryValue::List(l) => Ok(v_int(l.len() as i64)),
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
                QueryValue::Map(m) => Ok(v_list(m.keys().map(|k| v_str(k.clone())).collect())),
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
fn type_name(v: &QueryValue) -> &'static str {
    match v {
        QueryValue::Null => "null",
        QueryValue::Bool(_) => "bool",
        QueryValue::Int(_) => "int",
        QueryValue::F64(_) => "f64",
        QueryValue::Dec(_) => "dec",
        QueryValue::Big(_) => "big",
        QueryValue::Str(_) => "string",
        QueryValue::Bin(_) => "bytes",
        QueryValue::List(_) => "list",
        QueryValue::Set(_) => "set",
        QueryValue::Map(_) => "map",
    }
}

/// Walk `root` following `steps`. Each step is an `Int` (for `List` indexing,
/// with negative counting from the end) or a `Str` (for `Map` key lookup).
/// Returns `Ok(None)` for a structural miss, `Ok(Some(&value))` on success,
/// and `"type_mismatch"` only when a step is neither `Int` nor `Str`.
fn navigate<'a>(
    root: &'a QueryValue,
    steps: &[QueryValue],
) -> Result<Option<&'a QueryValue>, ScalarError> {
    let mut cur = root;
    for step in steps {
        match step {
            QueryValue::Int(idx) => {
                let idx = *idx;
                match cur {
                    QueryValue::List(l) => {
                        let len = l.len() as i64;
                        let resolved = if idx < 0 { len + idx } else { idx };
                        if resolved < 0 || resolved >= len {
                            return Ok(None);
                        }
                        cur = &l[resolved as usize];
                    }
                    QueryValue::Map(m) => {
                        // Integer step into a map: look up by the stringified
                        // integer (back-compat with callers that pass numeric
                        // keys).
                        let key = idx.to_string();
                        match m.get(&key) {
                            Some(v) => cur = v,
                            None => return Ok(None),
                        }
                    }
                    _ => return Ok(None),
                }
            }
            QueryValue::Str(key) => {
                match cur {
                    QueryValue::Map(m) => match m.get(key.as_str()) {
                        Some(v) => cur = v,
                        None => return Ok(None),
                    },
                    _ => return Ok(None),
                }
            }
            _ => return Err(ScalarError::new("type_mismatch")),
        }
    }
    Ok(Some(cur))
}
