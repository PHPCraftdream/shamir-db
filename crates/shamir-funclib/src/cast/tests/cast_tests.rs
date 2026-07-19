//! Per-function `/cast` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::cast;
use crate::registry::{v_bool, v_int, v_str, ScalarRegistry};
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    cast::register(&mut r);
    r
}

fn dec(s: &str) -> QueryValue {
    QueryValue::Dec(Decimal::from_str_exact(s).unwrap())
}

#[test]
fn to_int_ok_and_failures() {
    let r = reg();
    assert_eq!(r.call("to_int", &[dec("7")]).unwrap(), v_int(7));
    assert_eq!(
        r.call("to_int", &[QueryValue::Bool(true)]).unwrap(),
        v_int(1)
    );
    assert_eq!(
        r.call("to_int", &[QueryValue::Str("42".into())]).unwrap(),
        v_int(42)
    );
    // fractional decimal cannot be an integer
    assert_eq!(
        r.call("to_int", &[dec("2.5")]).unwrap_err().code,
        "cast_failed"
    );
    // non-numeric string
    assert_eq!(
        r.call("to_int", &[QueryValue::Str("xx".into())])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn to_float_ok_and_failure() {
    let r = reg();
    assert_eq!(r.call("to_float", &[QueryValue::Int(3)]).unwrap(), dec("3"));
    assert_eq!(
        r.call("to_float", &[QueryValue::Str("1.5".into())])
            .unwrap(),
        dec("1.5")
    );
    // unparsable string
    assert_eq!(
        r.call("to_float", &[QueryValue::Str("nope".into())])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn to_dec_ok_and_failure() {
    let r = reg();
    assert_eq!(r.call("to_dec", &[QueryValue::Int(5)]).unwrap(), dec("5"));
    assert_eq!(
        r.call("to_dec", &[QueryValue::Str("3.14".into())]).unwrap(),
        dec("3.14")
    );
    // List is not convertible
    assert_eq!(
        r.call("to_dec", &[QueryValue::List(vec![])])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn to_string_renders_variants() {
    let r = reg();
    assert_eq!(
        r.call("to_string", &[QueryValue::Int(-9)]).unwrap(),
        v_str("-9".to_string())
    );
    assert_eq!(
        r.call("to_string", &[QueryValue::Bool(true)]).unwrap(),
        v_str("true".to_string())
    );
    assert_eq!(
        r.call("to_string", &[dec("2.50")]).unwrap(),
        v_str("2.50".to_string())
    );
    // edge: missing arg -> arity
    assert_eq!(r.call("to_string", &[]).unwrap_err().code, "arity");
}

#[test]
fn to_bool_ok_and_failure() {
    let r = reg();
    assert_eq!(
        r.call("to_bool", &[QueryValue::Int(0)]).unwrap(),
        v_bool(false)
    );
    assert_eq!(
        r.call("to_bool", &[QueryValue::Int(5)]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("to_bool", &[QueryValue::Str("TRUE".into())])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("to_bool", &[QueryValue::Str("0".into())]).unwrap(),
        v_bool(false)
    );
    // unrecognised string
    assert_eq!(
        r.call("to_bool", &[QueryValue::Str("maybe".into())])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn parse_int_ok_and_failures() {
    let r = reg();
    assert_eq!(
        r.call("parse_int", &[QueryValue::Str("  100 ".into())])
            .unwrap(),
        v_int(100)
    );
    // malformed literal
    assert_eq!(
        r.call("parse_int", &[QueryValue::Str("1.5".into())])
            .unwrap_err()
            .code,
        "cast_failed"
    );
    // wrong type: requires Str
    assert_eq!(
        r.call("parse_int", &[QueryValue::Int(3)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn parse_float_ok_and_failure() {
    let r = reg();
    assert_eq!(
        r.call("parse_float", &[QueryValue::Str("2.718".into())])
            .unwrap(),
        dec("2.718")
    );
    // malformed literal
    assert_eq!(
        r.call("parse_float", &[QueryValue::Str("abc".into())])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn try_cast_dispatch_and_unknown_type() {
    let r = reg();
    assert_eq!(
        r.call(
            "try_cast",
            &[QueryValue::Str("42".into()), QueryValue::Str("int".into())]
        )
        .unwrap(),
        v_int(42)
    );
    assert_eq!(
        r.call("try_cast", &[dec("3.5"), QueryValue::Str("string".into())])
            .unwrap(),
        v_str("3.5".to_string())
    );
    assert_eq!(
        r.call(
            "try_cast",
            &[QueryValue::Int(0), QueryValue::Str("bool".into())]
        )
        .unwrap(),
        v_bool(false)
    );
    // unknown target type
    assert_eq!(
        r.call(
            "try_cast",
            &[QueryValue::Int(1), QueryValue::Str("uuid".into())]
        )
        .unwrap_err()
        .code,
        "unknown_type"
    );
    // underlying cast failure propagates
    assert_eq!(
        r.call(
            "try_cast",
            &[QueryValue::Str("x".into()), QueryValue::Str("int".into())]
        )
        .unwrap_err()
        .code,
        "cast_failed"
    );
}

// ---------------------------------------------------------------------------
// Big support in cast_to_int / cast_to_dec (mirrors agg::to_dec)
// ---------------------------------------------------------------------------

#[test]
fn cast_big_to_int_ok_and_overflow() {
    let r = reg();
    // Big(42) fits i64 → Int(42).
    assert_eq!(
        r.call("to_int", &[QueryValue::Big(BigInt::from(42))])
            .unwrap(),
        v_int(42)
    );
    // try_cast with "int" target.
    assert_eq!(
        r.call(
            "try_cast",
            &[
                QueryValue::Big(BigInt::from(42)),
                QueryValue::Str("int".into())
            ]
        )
        .unwrap(),
        v_int(42)
    );
    // Big(i64::MAX + 1) overflows i64 → cast_failed (not truncated/wrapped).
    assert_eq!(
        r.call("to_int", &[QueryValue::Big(BigInt::from(i64::MAX) + 1)])
            .unwrap_err()
            .code,
        "cast_failed"
    );
}

#[test]
fn cast_big_to_dec_exact_and_f64_fallback() {
    let r = reg();
    // Big(42) fits i64 → exact Decimal(42).
    assert_eq!(
        r.call("to_dec", &[QueryValue::Big(BigInt::from(42))])
            .unwrap(),
        dec("42")
    );
    // Big(i64::MAX + 1) doesn't fit i64 but fits f64/Decimal → succeeds via
    // the f64 fallback (same behaviour as agg::to_dec for the same input).
    let huge = QueryValue::Big(BigInt::from(i64::MAX) + 1);
    let result = r.call("to_dec", &[huge]).unwrap();
    assert!(matches!(result, QueryValue::Dec(_)));
    // A genuinely unrepresentable Big (far beyond f64 range) → cast_failed.
    // 10^400 cannot be converted to f64 or i64.
    let absurd = QueryValue::Big(BigInt::from(10).pow(400));
    assert_eq!(r.call("to_dec", &[absurd]).unwrap_err().code, "cast_failed");
}

#[test]
fn cast_big_to_float_is_dec() {
    let r = reg();
    // "float" target dispatches to cast_to_dec.
    assert_eq!(
        r.call(
            "try_cast",
            &[
                QueryValue::Big(BigInt::from(42)),
                QueryValue::Str("float".into())
            ]
        )
        .unwrap(),
        dec("42")
    );
    assert_eq!(
        r.call("to_float", &[QueryValue::Big(BigInt::from(42))])
            .unwrap(),
        dec("42")
    );
}
