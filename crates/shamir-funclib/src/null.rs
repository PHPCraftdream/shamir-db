//! `/null` scalar category — null-handling functions.
//!
//! Functions registered (plain names, folder-qualified to `null/…` by
//! [`crate::register_builtins`]): `coalesce if_null nullif is_null`.
//!
//! Conventions established here, mirroring [`crate::math`]:
//! - `coalesce(v…)` is variadic (≥1 arg, unbounded): returns the first
//!   non-null argument, or [`QueryValue::Null`] if every argument is null.
//!   A zero-argument call is rejected with `"arity"` by the registry's
//!   min-args gate (`min_args = 1`, exactly like `math::min`/`max`) — a
//!   coalesce over no values has nothing to return and would mask a caller
//!   bug. Unlike `min`/`max`, a coalesce over all-null args is **not** an
//!   error: it is a valid, common case and yields `Null`.
//! - `if_null(v, default)` is the 2-arg specialization of `coalesce`:
//!   returns `v` unchanged when non-null, otherwise `default` (which may
//!   itself be `Null`).
//! - `nullif(a, b)` is exactly 2 args: returns [`QueryValue::Null`] when
//!   `a` and `b` compare equal under [`crate::compare::compare`] (the
//!   workspace cross-type total order — so `nullif(Int(5), Dec(5.0))` is
//!   `Null`, not merely same-variant equality), otherwise returns `a`
//!   unchanged. Matches how `math::clamp`/`between` use `compare` for
//!   cross-type comparisons rather than `PartialEq`.
//! - `is_null(v)` is exactly 1 arg: returns `Bool(true)` iff `v` is
//!   [`QueryValue::Null`], else `Bool(false)`.

use crate::compare::compare;
use crate::registry::{v_bool, FnEntry, ScalarError, ScalarRegistry};
use shamir_types::types::value::QueryValue;
use std::cmp::Ordering;

/// Register the `/null` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "coalesce",
        FnEntry::pure(
            |a| {
                for v in a {
                    if !matches!(v, QueryValue::Null) {
                        return Ok(v.clone());
                    }
                }
                Ok(QueryValue::Null)
            },
            1,
            None,
        ),
    );
    reg.register(
        "if_null",
        FnEntry::pure(
            |a| {
                let v = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                let default = a.get(1).ok_or_else(|| ScalarError::new("missing_arg"))?;
                if matches!(v, QueryValue::Null) {
                    Ok(default.clone())
                } else {
                    Ok(v.clone())
                }
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "nullif",
        FnEntry::pure(
            |a| {
                let x = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                let y = a.get(1).ok_or_else(|| ScalarError::new("missing_arg"))?;
                if compare(x, y) == Ordering::Equal {
                    Ok(QueryValue::Null)
                } else {
                    Ok(x.clone())
                }
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "is_null",
        FnEntry::pure(
            |a| {
                let v = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                Ok(v_bool(matches!(v, QueryValue::Null)))
            },
            1,
            Some(1),
        ),
    );
}

#[cfg(test)]
mod tests;
