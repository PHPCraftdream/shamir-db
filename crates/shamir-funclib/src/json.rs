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
