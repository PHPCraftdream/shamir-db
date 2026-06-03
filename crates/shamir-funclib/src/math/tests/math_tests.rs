//! Per-function `/math` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::math;
use crate::registry::{v_bool, v_dec, v_int, ScalarRegistry};
use rust_decimal::Decimal;
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    math::register(&mut r);
    r
}

fn dec(s: &str) -> InnerValue {
    InnerValue::Dec(Decimal::from_str_exact(s).unwrap())
}

#[test]
fn abs_ok_and_neg() {
    let r = reg();
    assert_eq!(r.call("abs", &[dec("-3.5")]).unwrap(), dec("3.5"));
    assert_eq!(
        r.call("abs", &[InnerValue::Int(-7)]).unwrap(),
        v_dec(Decimal::from(7))
    );
    // error: wrong type
    assert_eq!(
        r.call("abs", &[InnerValue::Str("x".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn ceil_floor_trunc() {
    let r = reg();
    assert_eq!(r.call("ceil", &[dec("2.1")]).unwrap(), dec("3"));
    assert_eq!(r.call("floor", &[dec("2.9")]).unwrap(), dec("2"));
    assert_eq!(r.call("trunc", &[dec("-2.9")]).unwrap(), dec("-2"));
    // edge: ceil of integer is itself
    assert_eq!(r.call("ceil", &[dec("5")]).unwrap(), dec("5"));
}

#[test]
fn round_default_and_places() {
    let r = reg();
    assert_eq!(r.call("round", &[dec("2.5")]).unwrap(), dec("3"));
    assert_eq!(
        r.call("round", &[dec("2.345"), InnerValue::Int(2)])
            .unwrap(),
        dec("2.35")
    );
    // error: negative decimal places
    assert_eq!(
        r.call("round", &[dec("2.5"), InnerValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn sign_and_neg() {
    let r = reg();
    assert_eq!(r.call("sign", &[dec("-9")]).unwrap(), v_int(-1));
    assert_eq!(r.call("sign", &[dec("0")]).unwrap(), v_int(0));
    assert_eq!(r.call("sign", &[dec("4")]).unwrap(), v_int(1));
    assert_eq!(r.call("neg", &[dec("4")]).unwrap(), dec("-4"));
    // error: neg missing arg -> arity
    assert_eq!(r.call("neg", &[]).unwrap_err().code, "arity");
}

#[test]
fn pow_sqrt() {
    let r = reg();
    // 2^10 = 1024
    match r
        .call("pow", &[InnerValue::Int(2), InnerValue::Int(10)])
        .unwrap()
    {
        InnerValue::Dec(d) => assert_eq!(d, Decimal::from(1024)),
        other => panic!("expected Dec, got {other:?}"),
    }
    match r.call("sqrt", &[InnerValue::Int(9)]).unwrap() {
        InnerValue::Dec(d) => assert_eq!(d, Decimal::from(3)),
        other => panic!("expected Dec, got {other:?}"),
    }
    // domain error: sqrt of negative
    assert_eq!(
        r.call("sqrt", &[InnerValue::Int(-1)]).unwrap_err().code,
        "domain"
    );
}

#[test]
fn exp_ln_log() {
    let r = reg();
    // ln(1) == 0
    match r.call("ln", &[InnerValue::Int(1)]).unwrap() {
        InnerValue::Dec(d) => assert_eq!(d, Decimal::ZERO),
        other => panic!("expected Dec, got {other:?}"),
    }
    // exp(0) == 1
    match r.call("exp", &[InnerValue::Int(0)]).unwrap() {
        InnerValue::Dec(d) => assert_eq!(d, Decimal::from(1)),
        other => panic!("expected Dec, got {other:?}"),
    }
    // log base 10 of 1000 == 3
    match r.call("log", &[InnerValue::Int(1000)]).unwrap() {
        InnerValue::Dec(d) => assert_eq!(d.round(), Decimal::from(3)),
        other => panic!("expected Dec, got {other:?}"),
    }
    // log base 2 of 8 == 3
    match r
        .call("log", &[InnerValue::Int(8), InnerValue::Int(2)])
        .unwrap()
    {
        InnerValue::Dec(d) => assert_eq!(d.round(), Decimal::from(3)),
        other => panic!("expected Dec, got {other:?}"),
    }
    // domain error: ln(0)
    assert_eq!(
        r.call("ln", &[InnerValue::Int(0)]).unwrap_err().code,
        "domain"
    );
}

#[test]
fn modulo() {
    let r = reg();
    assert_eq!(
        r.call("mod", &[InnerValue::Int(10), InnerValue::Int(3)])
            .unwrap(),
        dec("1")
    );
    // div by zero
    assert_eq!(
        r.call("mod", &[InnerValue::Int(10), InnerValue::Int(0)])
            .unwrap_err()
            .code,
        "div_by_zero"
    );
}

#[test]
fn clamp() {
    let r = reg();
    assert_eq!(
        r.call("clamp", &[dec("5"), dec("1"), dec("3")]).unwrap(),
        dec("3")
    );
    assert_eq!(
        r.call("clamp", &[dec("0"), dec("1"), dec("3")]).unwrap(),
        dec("1")
    );
    assert_eq!(
        r.call("clamp", &[dec("2"), dec("1"), dec("3")]).unwrap(),
        dec("2")
    );
    // bad bounds: lo > hi
    assert_eq!(
        r.call("clamp", &[dec("2"), dec("3"), dec("1")])
            .unwrap_err()
            .code,
        "bad_bounds"
    );
}

#[test]
fn min_max_nary() {
    let r = reg();
    assert_eq!(
        r.call("min", &[dec("5"), dec("2"), dec("8"), dec("-1")])
            .unwrap(),
        dec("-1")
    );
    assert_eq!(
        r.call("max", &[dec("5"), dec("2"), dec("8"), dec("-1")])
            .unwrap(),
        dec("8")
    );
    // single arg ok
    assert_eq!(r.call("min", &[dec("42")]).unwrap(), dec("42"));
    // error: empty args -> arity (min_args = 1)
    assert_eq!(r.call("max", &[]).unwrap_err().code, "arity");
}

#[test]
fn between() {
    let r = reg();
    assert_eq!(
        r.call("between", &[dec("5"), dec("1"), dec("10")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("between", &[dec("11"), dec("1"), dec("10")])
            .unwrap(),
        v_bool(false)
    );
    // inclusive edges
    assert_eq!(
        r.call("between", &[dec("1"), dec("1"), dec("10")]).unwrap(),
        v_bool(true)
    );
    // bad bounds
    assert_eq!(
        r.call("between", &[dec("5"), dec("10"), dec("1")])
            .unwrap_err()
            .code,
        "bad_bounds"
    );
}

#[test]
fn min_max_cross_type() {
    let r = reg();
    // Numbers (rank 2) < Strings (rank 3), so Int 5 < Str "a".
    assert_eq!(
        r.call("min", &[InnerValue::Int(5), InnerValue::Str("a".into())])
            .unwrap(),
        InnerValue::Int(5)
    );
    assert_eq!(
        r.call("max", &[InnerValue::Int(5), InnerValue::Str("a".into())])
            .unwrap(),
        InnerValue::Str("a".into())
    );
    // Bool (rank 1) < Int (rank 2) < Str (rank 3).
    assert_eq!(
        r.call(
            "min",
            &[
                InnerValue::Str("z".into()),
                InnerValue::Int(10),
                InnerValue::Bool(true)
            ]
        )
        .unwrap(),
        InnerValue::Bool(true)
    );
}

#[test]
fn clamp_cross_type() {
    let r = reg();
    // Clamp a Bool (rank 1) into [Int 0, Str "z"] -> Bool stays (between ranks).
    assert_eq!(
        r.call(
            "clamp",
            &[
                InnerValue::Bool(false),
                InnerValue::Int(0),
                InnerValue::Str("z".into())
            ]
        )
        .unwrap(),
        // Bool rank 1 < Int rank 2 (lo), so clamped up to lo.
        InnerValue::Int(0)
    );
}

#[test]
fn between_cross_type() {
    let r = reg();
    // Int 5 is between Bool false (rank 1) and Str "a" (rank 3).
    assert_eq!(
        r.call(
            "between",
            &[
                InnerValue::Int(5),
                InnerValue::Bool(false),
                InnerValue::Str("a".into())
            ]
        )
        .unwrap(),
        v_bool(true)
    );
    // Bool false is NOT between Int 1 and Str "z" (Bool rank 1 < Int rank 2 = lo).
    assert_eq!(
        r.call(
            "between",
            &[
                InnerValue::Bool(false),
                InnerValue::Int(1),
                InnerValue::Str("z".into())
            ]
        )
        .unwrap(),
        v_bool(false)
    );
}
