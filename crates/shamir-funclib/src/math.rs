//! `/math` scalar category — the **reference implementation** every other
//! category module copies.
//!
//! Functions registered (plain names, no folder prefix):
//! `abs ceil floor round trunc sign neg pow sqrt exp ln log mod clamp
//!  min max between`.
//!
//! Conventions established here for the rest of the library:
//! - Exact ops (`abs`, `ceil`, `floor`, `round`, `trunc`, `sign`, `neg`,
//!   `mod`) operate on [`Decimal`] via [`arg_dec`] and return a `Dec`,
//!   preserving precision across the numeric variants.
//! - Transcendental ops (`pow`, `sqrt`, `exp`, `ln`, `log`) compute in `f64`
//!   and return a `Dec` via [`v_f64`]; domain errors (e.g. `sqrt(-1)`,
//!   `ln(0)`) yield `ScalarError("domain")`.
//! - `min`, `max`, `clamp`, and `between` use [`crate::compare::compare`]
//!   (cross-type total order) so they work across ANY value types, returning
//!   the original value as-is (no numeric coercion).
//! - `sign` returns an `Int` (-1/0/1); `between` returns a `Bool`.

use crate::compare::compare;
use crate::registry::{
    arg_dec, arg_f64, v_bool, v_dec, v_f64, v_int, FnEntry, ScalarError, ScalarRegistry,
    ScalarResult,
};
use rust_decimal::RoundingStrategy;
use shamir_types::types::value::InnerValue;
use std::cmp::Ordering;

/// Register the `/math` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "abs",
        FnEntry::pure(|a| Ok(v_dec(arg_dec(a, 0)?.abs())), 1, Some(1)),
    );
    reg.register(
        "ceil",
        FnEntry::pure(|a| Ok(v_dec(arg_dec(a, 0)?.ceil())), 1, Some(1)),
    );
    reg.register(
        "floor",
        FnEntry::pure(|a| Ok(v_dec(arg_dec(a, 0)?.floor())), 1, Some(1)),
    );
    reg.register(
        "round",
        FnEntry::pure(
            |a| {
                let x = arg_dec(a, 0)?;
                // Optional second arg: number of decimal places (>= 0).
                let v = if a.len() >= 2 {
                    let dp = crate::registry::arg_i64(a, 1)?;
                    if !(0..=28).contains(&dp) {
                        return Err(ScalarError::new("out_of_range"));
                    }
                    x.round_dp_with_strategy(dp as u32, RoundingStrategy::MidpointAwayFromZero)
                } else {
                    x.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
                };
                Ok(v_dec(v))
            },
            1,
            Some(2),
        ),
    );
    reg.register(
        "trunc",
        FnEntry::pure(|a| Ok(v_dec(arg_dec(a, 0)?.trunc())), 1, Some(1)),
    );
    reg.register(
        "sign",
        FnEntry::pure(
            |a| {
                let x = arg_dec(a, 0)?;
                let s = if x.is_zero() {
                    0
                } else if x.is_sign_negative() {
                    -1
                } else {
                    1
                };
                Ok(v_int(s))
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "neg",
        FnEntry::pure(|a| Ok(v_dec(-arg_dec(a, 0)?)), 1, Some(1)),
    );
    reg.register(
        "pow",
        FnEntry::pure(
            |a| {
                let base = arg_f64(a, 0)?;
                let exp = arg_f64(a, 1)?;
                let r = base.powf(exp);
                if r.is_finite() {
                    v_f64(r)
                } else {
                    Err(ScalarError::new("domain"))
                }
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "sqrt",
        FnEntry::pure(
            |a| {
                let x = arg_f64(a, 0)?;
                if x < 0.0 {
                    return Err(ScalarError::new("domain"));
                }
                v_f64(x.sqrt())
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "exp",
        FnEntry::pure(
            |a| {
                let r = arg_f64(a, 0)?.exp();
                if r.is_finite() {
                    v_f64(r)
                } else {
                    Err(ScalarError::new("domain"))
                }
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "ln",
        FnEntry::pure(
            |a| {
                let x = arg_f64(a, 0)?;
                if x <= 0.0 {
                    return Err(ScalarError::new("domain"));
                }
                v_f64(x.ln())
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "log",
        FnEntry::pure(
            |a| {
                // log(x) base 10, or log(x, base) with explicit base.
                let x = arg_f64(a, 0)?;
                if x <= 0.0 {
                    return Err(ScalarError::new("domain"));
                }
                if a.len() >= 2 {
                    let base = arg_f64(a, 1)?;
                    if base <= 0.0 || base == 1.0 {
                        return Err(ScalarError::new("domain"));
                    }
                    v_f64(x.log(base))
                } else {
                    v_f64(x.log10())
                }
            },
            1,
            Some(2),
        ),
    );
    reg.register(
        "mod",
        FnEntry::pure(
            |a| {
                let x = arg_dec(a, 0)?;
                let m = arg_dec(a, 1)?;
                if m.is_zero() {
                    return Err(ScalarError::new("div_by_zero"));
                }
                Ok(v_dec(x % m))
            },
            2,
            Some(2),
        ),
    );
    reg.register(
        "clamp",
        FnEntry::pure(
            |a| {
                let x = a
                    .first()
                    .ok_or_else(|| ScalarError::new("missing_arg"))?
                    .clone();
                let lo = a
                    .get(1)
                    .ok_or_else(|| ScalarError::new("missing_arg"))?
                    .clone();
                let hi = a
                    .get(2)
                    .ok_or_else(|| ScalarError::new("missing_arg"))?
                    .clone();
                if compare(&lo, &hi) == Ordering::Greater {
                    return Err(ScalarError::new("bad_bounds"));
                }
                if compare(&x, &lo) == Ordering::Less {
                    Ok(lo)
                } else if compare(&x, &hi) == Ordering::Greater {
                    Ok(hi)
                } else {
                    Ok(x)
                }
            },
            3,
            Some(3),
        ),
    );
    reg.register("min", FnEntry::pure(|a| reduce(a, Reduce::Min), 1, None));
    reg.register("max", FnEntry::pure(|a| reduce(a, Reduce::Max), 1, None));
    reg.register(
        "between",
        FnEntry::pure(
            |a| {
                let x = a.first().ok_or_else(|| ScalarError::new("missing_arg"))?;
                let lo = a.get(1).ok_or_else(|| ScalarError::new("missing_arg"))?;
                let hi = a.get(2).ok_or_else(|| ScalarError::new("missing_arg"))?;
                if compare(lo, hi) == Ordering::Greater {
                    return Err(ScalarError::new("bad_bounds"));
                }
                Ok(v_bool(
                    compare(x, lo) != Ordering::Less && compare(x, hi) != Ordering::Greater,
                ))
            },
            3,
            Some(3),
        ),
    );
}

enum Reduce {
    Min,
    Max,
}

/// N-ary min/max over all arguments using cross-type [`compare`]. Returns the
/// winning argument as-is (no numeric coercion), so mixed types work.
fn reduce(args: &[InnerValue], kind: Reduce) -> ScalarResult {
    let mut acc = args
        .first()
        .ok_or_else(|| ScalarError::new("missing_arg"))?
        .clone();
    for v in &args[1..] {
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
        };
    }
    Ok(acc)
}

#[cfg(test)]
mod tests;
