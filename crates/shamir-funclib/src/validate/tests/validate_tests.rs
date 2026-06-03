//! Per-function `/validate` tests — at least one correct-result assert and one
//! error/edge case per registered function.

use crate::registry::{v_bool, ScalarRegistry};
use crate::validate;
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    validate::register(&mut r);
    r
}

fn s(x: &str) -> InnerValue {
    InnerValue::Str(x.into())
}

#[test]
fn is_email_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_email", &[s("a.b+c@ex-ample.com")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_email", &[s("not-an-email")]).unwrap(),
        v_bool(false)
    );
    // wrong type
    assert_eq!(
        r.call("is_email", &[InnerValue::Int(1)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn is_url_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_url", &[s("https://example.com/path?q=1")])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_url", &[s("ftp://example.com")]).unwrap(),
        v_bool(false)
    );
    // arity
    assert_eq!(r.call("is_url", &[]).unwrap_err().code, "arity");
}

#[test]
fn is_uuid_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_uuid", &[s("550e8400-e29b-41d4-a716-446655440000")])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_uuid", &[s("550e8400e29b41d4")]).unwrap(),
        v_bool(false)
    );
}

#[test]
fn is_ipv4_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_ipv4", &[s("192.168.0.1")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(r.call("is_ipv4", &[s("256.0.0.1")]).unwrap(), v_bool(false));
    assert_eq!(r.call("is_ipv4", &[s("1.2.3")]).unwrap(), v_bool(false));
}

#[test]
fn is_ipv6_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_ipv6", &[s("2001:db8::ff00:42:8329")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(r.call("is_ipv6", &[s("::1")]).unwrap(), v_bool(true));
    assert_eq!(r.call("is_ipv6", &[s("not:ipv6")]).unwrap(), v_bool(false));
}

#[test]
fn is_phone_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_phone", &[s("+14155552671")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_phone", &[s("14155552671")]).unwrap(),
        v_bool(true)
    );
    // too short / contains letters
    assert_eq!(r.call("is_phone", &[s("12345")]).unwrap(), v_bool(false));
    assert_eq!(r.call("is_phone", &[s("+1abc")]).unwrap(), v_bool(false));
}

#[test]
fn luhn_ok_and_bad() {
    let r = reg();
    // Valid test card number (Visa test).
    assert_eq!(
        r.call("luhn", &[s("4242424242424242")]).unwrap(),
        v_bool(true)
    );
    // One digit off -> invalid.
    assert_eq!(
        r.call("luhn", &[s("4242424242424241")]).unwrap(),
        v_bool(false)
    );
    // Non-digit -> invalid.
    assert_eq!(r.call("luhn", &[s("4242-4242")]).unwrap(), v_bool(false));
    // Empty -> invalid.
    assert_eq!(r.call("luhn", &[s("")]).unwrap(), v_bool(false));
}

#[test]
fn in_range_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call(
            "in_range",
            &[InnerValue::Int(5), InnerValue::Int(1), InnerValue::Int(10)]
        )
        .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call(
            "in_range",
            &[InnerValue::Int(11), InnerValue::Int(1), InnerValue::Int(10)]
        )
        .unwrap(),
        v_bool(false)
    );
    // inclusive edge
    assert_eq!(
        r.call(
            "in_range",
            &[InnerValue::Int(1), InnerValue::Int(1), InnerValue::Int(10)]
        )
        .unwrap(),
        v_bool(true)
    );
    // bad bounds: lo > hi
    assert_eq!(
        r.call(
            "in_range",
            &[InnerValue::Int(5), InnerValue::Int(10), InnerValue::Int(1)]
        )
        .unwrap_err()
        .code,
        "bad_bounds"
    );
}

#[test]
fn matches_ok_and_bad_pattern() {
    let r = reg();
    assert_eq!(
        r.call("matches", &[s("abc123"), s(r"^[a-z]+\d+$")])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("matches", &[s("ABC"), s(r"^[a-z]+$")]).unwrap(),
        v_bool(false)
    );
    // invalid regex -> machine code
    assert_eq!(
        r.call("matches", &[s("x"), s("(")]).unwrap_err().code,
        "bad_pattern"
    );
}

#[test]
fn is_json_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call("is_json", &[s(r#"{"a": [1, 2, true, null], "b": "xA"}"#)])
            .unwrap(),
        v_bool(true)
    );
    assert_eq!(r.call("is_json", &[s("123")]).unwrap(), v_bool(true));
    assert_eq!(r.call("is_json", &[s(r#""hello""#)]).unwrap(), v_bool(true));
    // trailing garbage / malformed
    assert_eq!(r.call("is_json", &[s("{a:1}")]).unwrap(), v_bool(false));
    assert_eq!(r.call("is_json", &[s("[1, 2,]")]).unwrap(), v_bool(false));
    assert_eq!(r.call("is_json", &[s("{} extra")]).unwrap(), v_bool(false));
}

#[test]
fn is_empty_across_variants() {
    let r = reg();
    assert_eq!(
        r.call("is_empty", &[InnerValue::Null]).unwrap(),
        v_bool(true)
    );
    assert_eq!(r.call("is_empty", &[s("")]).unwrap(), v_bool(true));
    assert_eq!(r.call("is_empty", &[s("x")]).unwrap(), v_bool(false));
    assert_eq!(
        r.call("is_empty", &[InnerValue::List(vec![])]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_empty", &[InnerValue::List(vec![InnerValue::Int(1)])])
            .unwrap(),
        v_bool(false)
    );
    // numbers are never empty
    assert_eq!(
        r.call("is_empty", &[InnerValue::Int(0)]).unwrap(),
        v_bool(false)
    );
    // arity
    assert_eq!(r.call("is_empty", &[]).unwrap_err().code, "arity");
}

#[test]
fn len_between_ok_and_bad() {
    let r = reg();
    assert_eq!(
        r.call(
            "len_between",
            &[s("hello"), InnerValue::Int(1), InnerValue::Int(10)]
        )
        .unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call(
            "len_between",
            &[s("hello"), InnerValue::Int(1), InnerValue::Int(3)]
        )
        .unwrap(),
        v_bool(false)
    );
    // counts chars, not bytes
    assert_eq!(
        r.call(
            "len_between",
            &[s("é"), InnerValue::Int(1), InnerValue::Int(1)]
        )
        .unwrap(),
        v_bool(true)
    );
    // bad bounds
    assert_eq!(
        r.call(
            "len_between",
            &[s("x"), InnerValue::Int(5), InnerValue::Int(1)]
        )
        .unwrap_err()
        .code,
        "bad_bounds"
    );
}
