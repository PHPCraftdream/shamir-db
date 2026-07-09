//! `/arrays` scalar category — list operations over a single record's `List`
//! value.
//!
//! Functions registered (plain names, no folder prefix):
//! `length get slice contains index_of first last flatten distinct sort join
//!  sum min max avg`.
//!
//! Conventions (mirroring [`crate::math`]):
//! - The array argument is always a `List`, extracted via [`arg_list`]; a
//!   non-list yields `ScalarError("type_mismatch")`.
//! - Index / length arguments go through [`arg_i64`]; a negative value yields
//!   `ScalarError("out_of_range")`.
//! - `min` and `max` use [`crate::compare::compare`] (cross-type total order),
//!   returning the winning element as-is; an empty array yields
//!   `ScalarError("empty")`.
//! - The numeric reductions (`sum`, `avg`) coerce each element to [`Decimal`]
//!   via [`arg_dec`] and return a `Dec`, preserving precision; a non-numeric
//!   element yields `type_mismatch` and an empty array yields
//!   `ScalarError("empty")`.
//! - `sort` orders elements by their decimal value (numeric arrays only).
//! - Accessors over an empty array (`first`, `last`) yield
//!   `ScalarError("empty")`; out-of-bounds `get` yields `out_of_range`.
//!
//! Every function here is pure + deterministic (indexable).

use crate::compare::compare;
use crate::registry::{
    arg_dec, arg_i64, arg_list, arg_str, v_bool, v_dec, v_int, v_list, v_str, FnEntry, ScalarError,
    ScalarRegistry,
};
use rust_decimal::Decimal;
use shamir_collections::new_fx_set_wc;
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;

/// Register the `/arrays` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "length",
        FnEntry::pure(|a| Ok(v_int(arg_list(a, 0)?.len() as i64)), 1, Some(1)),
    );
    reg.register(
        "get",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let i = arg_i64(a, 1)?;
                if i < 0 || i as usize >= arr.len() {
                    return Err(ScalarError::new("out_of_range"));
                }
                Ok(arr[i as usize].clone())
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "slice",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let start = arg_i64(a, 1)?;
                let len = arg_i64(a, 2)?;
                if start < 0 || len < 0 {
                    return Err(ScalarError::new("out_of_range"));
                }
                let start = (start as usize).min(arr.len());
                let end = start.saturating_add(len as usize).min(arr.len());
                Ok(v_list(arr[start..end].to_vec()))
            },
            3,
            Some(3),
        ),
    );
    reg.register(
        "contains",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let needle = a.get(1).ok_or_else(|| ScalarError::new("missing_arg"))?;
                Ok(v_bool(arr.iter().any(|e| e == needle)))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "index_of",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let needle = a.get(1).ok_or_else(|| ScalarError::new("missing_arg"))?;
                let idx = arr
                    .iter()
                    .position(|e| e == needle)
                    .map(|p| p as i64)
                    .unwrap_or(-1);
                Ok(v_int(idx))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "first",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                arr.first()
                    .cloned()
                    .ok_or_else(|| ScalarError::new("empty"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "last",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                arr.last().cloned().ok_or_else(|| ScalarError::new("empty"))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "flatten",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let mut out: Vec<QueryValue> = Vec::new();
                for e in arr {
                    match e {
                        QueryValue::List(inner) => out.extend(inner.iter().cloned()),
                        _ => return Err(ScalarError::new("type_mismatch")),
                    }
                }
                Ok(v_list(out))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "distinct",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                // O(N) order-preserving dedup. Membership test is O(1) amortised
                // via an `FxHasher`-keyed set (`TFxSet`) instead of the legacy
                // O(N) linear scan of the kept-so-far vector (which made the
                // whole call O(N²) full `PartialEq` comparisons on
                // `QueryValue`). The output order (first occurrence) is
                // preserved by only pushing into `out` on the same first-sight
                // that succeeded in `seen`.
                //
                // Semantics: uses `QueryValue`'s existing `Hash`/`Eq` impls
                // (`Value<Key>` hand-hashes floats via `f.to_bits()`). That is
                // consistent with `Eq` for the canonical bit-pattern NaN case
                // and matches the legacy `==` behaviour for every other variant.
                let mut out: Vec<QueryValue> = Vec::with_capacity(arr.len());
                let mut seen = new_fx_set_wc(arr.len());
                for e in arr {
                    if seen.insert(e.clone()) {
                        out.push(e.clone());
                    }
                }
                Ok(v_list(out))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "sort",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                // Numeric sort by decimal value; non-numeric element -> type_mismatch.
                let mut keyed: Vec<(Decimal, QueryValue)> = Vec::with_capacity(arr.len());
                for i in 0..arr.len() {
                    keyed.push((arg_dec(arr, i)?, arr[i].clone()));
                }
                keyed.sort_by(|x, y| x.0.cmp(&y.0));
                Ok(v_list(keyed.into_iter().map(|(_, v)| v).collect()))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "join",
        FnEntry::pure(
            |a| {
                let arr = arg_list(a, 0)?;
                let sep = arg_str(a, 1)?;
                let mut parts: Vec<&str> = Vec::with_capacity(arr.len());
                for e in arr {
                    match e {
                        QueryValue::Str(s) => parts.push(s.as_str()),
                        _ => return Err(ScalarError::new("type_mismatch")),
                    }
                }
                Ok(v_str(parts.join(sep)))
            },
            2,
            Some(2),
        ),
    );
    reg.register("sum", FnEntry::pure(|a| reduce(a, Reduce::Sum), 1, Some(1)));
    reg.register("min", FnEntry::pure(|a| reduce(a, Reduce::Min), 1, Some(1)));
    reg.register("max", FnEntry::pure(|a| reduce(a, Reduce::Max), 1, Some(1)));
    reg.register("avg", FnEntry::pure(|a| reduce(a, Reduce::Avg), 1, Some(1)));
}

enum Reduce {
    Sum,
    Min,
    Max,
    Avg,
}

/// Reduction over the elements of the single `List` argument.
///
/// `Min`/`Max` use [`compare`] (cross-type total order), returning the element
/// as-is. `Sum`/`Avg` coerce each element via [`arg_dec`] and stay numeric.
/// An empty array yields `"empty"`.
fn reduce(args: &[QueryValue], kind: Reduce) -> crate::registry::ScalarResult {
    let arr = arg_list(args, 0)?;
    if arr.is_empty() {
        return Err(ScalarError::new("empty"));
    }
    match kind {
        Reduce::Min | Reduce::Max => {
            let mut acc = arr[0].clone();
            for v in &arr[1..] {
                let ord = compare(&acc, v);
                acc = match kind {
                    Reduce::Min => {
                        if ord == Ordering::Greater {
                            v.clone()
                        } else {
                            acc
                        }
                    }
                    Reduce::Max => {
                        if ord == Ordering::Less {
                            v.clone()
                        } else {
                            acc
                        }
                    }
                    _ => unreachable!(),
                };
            }
            Ok(acc)
        }
        Reduce::Sum | Reduce::Avg => {
            let mut acc: Decimal = arg_dec(arr, 0)?;
            for i in 1..arr.len() {
                acc += arg_dec(arr, i)?;
            }
            if let Reduce::Avg = kind {
                acc /= Decimal::from(arr.len());
            }
            Ok(v_dec(acc))
        }
    }
}

#[cfg(test)]
mod tests;
