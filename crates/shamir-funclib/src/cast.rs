//! `/cast` scalar category — type-conversion functions.
//!
//! Functions registered (plain names, no folder prefix):
//! `to_int to_float to_dec to_string to_bool parse_int parse_float try_cast`.
//!
//! Conventions (mirroring [`crate::math`]):
//! - Coercing casts (`to_int`, `to_float`, `to_dec`, `to_bool`) accept any
//!   numeric variant plus `Bool` and `Str`, parsing strings where it makes
//!   sense; an unconvertible value yields `ScalarError("cast_failed")`.
//! - `to_int` returns `Int`; `to_float`/`to_dec` return `Dec` (decimal-first
//!   value model); `to_string` returns `Str`; `to_bool` returns `Bool`.
//! - `parse_int`/`parse_float` require a `Str` argument and fail with
//!   `"cast_failed"` on a malformed literal.
//! - `try_cast(value, type_name)` dispatches on a target-type name string
//!   (`"int" "float" "dec" "string" "bool"`); an unknown name yields
//!   `ScalarError("unknown_type")`.
//!
//! Every function is pure + deterministic (no clock/env access), so each may
//! back a functional index.

use crate::registry::{v_bool, v_dec, v_int, v_str, FnEntry, ScalarError, ScalarRegistry};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;
use std::str::FromStr;

/// Register the `/cast` functions.
pub fn register(reg: &mut ScalarRegistry) {
    reg.register(
        "to_int",
        FnEntry::pure(|a| cast_to_int(arg(a, 0)?), 1, Some(1)),
    );
    reg.register(
        "to_float",
        FnEntry::pure(|a| cast_to_dec(arg(a, 0)?), 1, Some(1)),
    );
    reg.register(
        "to_dec",
        FnEntry::pure(|a| cast_to_dec(arg(a, 0)?), 1, Some(1)),
    );
    reg.register(
        "to_string",
        FnEntry::pure(|a| Ok(v_str(stringify(arg(a, 0)?))), 1, Some(1)),
    );
    reg.register(
        "to_bool",
        FnEntry::pure(|a| cast_to_bool(arg(a, 0)?), 1, Some(1)),
    );
    reg.register(
        "parse_int",
        FnEntry::pure(
            |a| {
                let s = as_str(arg(a, 0)?)?;
                parse_int_str(s)
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "parse_float",
        FnEntry::pure(
            |a| {
                let s = as_str(arg(a, 0)?)?;
                parse_dec_str(s)
            },
            1,
            Some(1),
        ),
    );
    reg.register(
        "try_cast",
        FnEntry::pure(
            |a| {
                let value = arg(a, 0)?;
                let ty = as_str(arg(a, 1)?)?;
                match ty {
                    "int" => cast_to_int(value),
                    "float" => cast_to_dec(value),
                    "dec" => cast_to_dec(value),
                    "string" => Ok(v_str(stringify(value))),
                    "bool" => cast_to_bool(value),
                    _ => Err(ScalarError::new("unknown_type")),
                }
            },
            2,
            Some(2),
        ),
    );
}

/// Fetch the `i`-th argument by reference or `ScalarError("missing_arg")`.
fn arg(args: &[QueryValue], i: usize) -> Result<&QueryValue, ScalarError> {
    args.get(i).ok_or_else(|| ScalarError::new("missing_arg"))
}

/// Borrow a `&str` from a `Str` value, else `"type_mismatch"`.
fn as_str(v: &QueryValue) -> Result<&str, ScalarError> {
    match v {
        QueryValue::Str(s) => Ok(s.as_str()),
        _ => Err(ScalarError::new("type_mismatch")),
    }
}

/// Coerce any value to an `Int`. Strings are parsed; fractional/out-of-range
/// numerics and unconvertible variants yield `"cast_failed"`.
fn cast_to_int(v: &QueryValue) -> Result<QueryValue, ScalarError> {
    match v {
        QueryValue::Int(n) => Ok(v_int(*n)),
        QueryValue::Bool(b) => Ok(v_int(*b as i64)),
        QueryValue::Dec(d) => {
            if d.fract().is_zero() {
                d.to_i64()
                    .map(v_int)
                    .ok_or_else(|| ScalarError::new("cast_failed"))
            } else {
                Err(ScalarError::new("cast_failed"))
            }
        }
        QueryValue::F64(f) => {
            if f.fract() == 0.0 && f.is_finite() && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                Ok(v_int(*f as i64))
            } else {
                Err(ScalarError::new("cast_failed"))
            }
        }
        QueryValue::Big(b) => b
            .to_i64()
            .map(v_int)
            .ok_or_else(|| ScalarError::new("cast_failed")),
        QueryValue::Str(s) => parse_int_str(s),
        _ => Err(ScalarError::new("cast_failed")),
    }
}

/// Coerce any value to a `Dec`. Strings are parsed; non-finite `F64` and
/// unconvertible variants yield `"cast_failed"`.
fn cast_to_dec(v: &QueryValue) -> Result<QueryValue, ScalarError> {
    match v {
        QueryValue::Dec(d) => Ok(v_dec(*d)),
        QueryValue::Int(n) => Ok(v_dec(Decimal::from(*n))),
        QueryValue::Bool(b) => Ok(v_dec(Decimal::from(*b as i64))),
        QueryValue::F64(f) => Decimal::from_f64_retain(*f)
            .map(v_dec)
            .ok_or_else(|| ScalarError::new("cast_failed")),
        QueryValue::Big(b) => {
            // Mirror agg::to_dec: i64 first (exact), then f64 fallback (lossy),
            // then error. BigInt has arbitrary precision so only values that
            // genuinely don't fit as a Decimal fail here.
            if let Some(n) = b.to_i64() {
                Ok(v_dec(Decimal::from(n)))
            } else if let Some(f) = b.to_f64() {
                Decimal::from_f64_retain(f)
                    .map(v_dec)
                    .ok_or_else(|| ScalarError::new("cast_failed"))
            } else {
                Err(ScalarError::new("cast_failed"))
            }
        }
        QueryValue::Str(s) => parse_dec_str(s),
        _ => Err(ScalarError::new("cast_failed")),
    }
}

/// Coerce any value to a `Bool`. Numerics map nonzero→true; strings accept
/// `true`/`false`/`1`/`0` (case-insensitive); anything else `"cast_failed"`.
fn cast_to_bool(v: &QueryValue) -> Result<QueryValue, ScalarError> {
    match v {
        QueryValue::Bool(b) => Ok(v_bool(*b)),
        QueryValue::Int(n) => Ok(v_bool(*n != 0)),
        QueryValue::Dec(d) => Ok(v_bool(!d.is_zero())),
        QueryValue::F64(f) => Ok(v_bool(*f != 0.0)),
        QueryValue::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(v_bool(true)),
            "false" | "0" => Ok(v_bool(false)),
            _ => Err(ScalarError::new("cast_failed")),
        },
        _ => Err(ScalarError::new("cast_failed")),
    }
}

/// Render any value to its canonical string form.
fn stringify(v: &QueryValue) -> String {
    match v {
        QueryValue::Null => "null".to_string(),
        QueryValue::Bool(b) => b.to_string(),
        QueryValue::Int(n) => n.to_string(),
        QueryValue::F64(f) => f.to_string(),
        QueryValue::Dec(d) => d.to_string(),
        QueryValue::Big(b) => b.to_string(),
        QueryValue::Str(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Parse a decimal-integer string into an `Int`.
fn parse_int_str(s: &str) -> Result<QueryValue, ScalarError> {
    s.trim()
        .parse::<i64>()
        .map(v_int)
        .map_err(|_| ScalarError::new("cast_failed"))
}

/// Parse a decimal string into a `Dec`.
fn parse_dec_str(s: &str) -> Result<QueryValue, ScalarError> {
    Decimal::from_str(s.trim())
        .map(v_dec)
        .map_err(|_| ScalarError::new("cast_failed"))
}

#[cfg(test)]
mod tests;
