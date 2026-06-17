//! Registry contract tests: dispatch, arity, unknown-function, and the value
//! extractor/constructor helpers shared by all categories.

use crate::math;
use crate::registry::{
    arg_bool, arg_bytes, arg_dec, arg_f64, arg_i64, arg_list, arg_str, v_bool, v_bytes, v_dec,
    v_f64, v_int, v_list, v_str, FnEntry, ScalarRegistry,
};
use rust_decimal::Decimal;
use shamir_types::types::value::QueryValue;

#[test]
fn unknown_function() {
    let r = ScalarRegistry::new();
    assert_eq!(r.call("nope", &[]).unwrap_err().code, "unknown_function");
}

#[test]
fn arity_bounds() {
    let mut r = ScalarRegistry::new();
    math::register(&mut r);
    // abs requires exactly 1 arg
    assert_eq!(r.call("abs", &[]).unwrap_err().code, "arity");
    assert_eq!(
        r.call("abs", &[QueryValue::Int(1), QueryValue::Int(2)])
            .unwrap_err()
            .code,
        "arity"
    );
}

#[test]
fn register_get_names_len() {
    let mut r = ScalarRegistry::new();
    assert!(r.is_empty());
    r.register("noop", FnEntry::pure(|_| Ok(v_int(0)), 0, Some(0)));
    assert_eq!(r.len(), 1);
    assert!(r.get("noop").is_some());
    assert_eq!(r.names(), vec!["noop"]);
    let e = r.get("noop").unwrap();
    assert!(e.pure && e.deterministic);
}

#[test]
fn extractors_ok() {
    let i = [QueryValue::Int(7)];
    assert_eq!(arg_i64(&i, 0).unwrap(), 7);
    assert_eq!(arg_f64(&i, 0).unwrap(), 7.0);
    assert_eq!(arg_dec(&i, 0).unwrap(), Decimal::from(7));

    let s = [QueryValue::Str("hi".into())];
    assert_eq!(arg_str(&s, 0).unwrap(), "hi");

    let b = [QueryValue::Bool(true)];
    assert!(arg_bool(&b, 0).unwrap());
    // bool coerces to i64/f64
    assert_eq!(arg_i64(&b, 0).unwrap(), 1);

    let l = [QueryValue::List(vec![QueryValue::Int(1)])];
    assert_eq!(arg_list(&l, 0).unwrap().len(), 1);

    let by = [QueryValue::Bin(vec![1, 2, 3])];
    assert_eq!(arg_bytes(&by, 0).unwrap(), &[1, 2, 3]);
}

#[test]
fn extractors_errors() {
    let i = [QueryValue::Int(7)];
    // missing arg
    assert_eq!(arg_i64(&i, 5).unwrap_err().code, "missing_arg");
    // type mismatch
    assert_eq!(arg_str(&i, 0).unwrap_err().code, "type_mismatch");
    // out of range: fractional decimal -> i64
    let d = [QueryValue::Dec(Decimal::from_str_exact("1.5").unwrap())];
    assert_eq!(arg_i64(&d, 0).unwrap_err().code, "out_of_range");
}

#[test]
fn constructors() {
    assert_eq!(v_int(3), QueryValue::Int(3));
    assert_eq!(v_str("x".into()), QueryValue::Str("x".into()));
    assert_eq!(v_bool(true), QueryValue::Bool(true));
    assert_eq!(v_dec(Decimal::from(2)), QueryValue::Dec(Decimal::from(2)));
    assert_eq!(
        v_list(vec![v_int(1)]),
        QueryValue::List(vec![QueryValue::Int(1)])
    );
    assert_eq!(v_bytes(vec![9]), QueryValue::Bin(vec![9]));
    // v_f64 stores as Dec; non-finite is an error
    assert!(matches!(v_f64(1.5).unwrap(), QueryValue::Dec(_)));
    assert_eq!(v_f64(f64::NAN).unwrap_err().code, "out_of_range");
}
