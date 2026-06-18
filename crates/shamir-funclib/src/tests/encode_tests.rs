use crate::encode;
use crate::registry::ScalarRegistry;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    encode::register(&mut r);
    r
}

fn s(v: &str) -> QueryValue {
    QueryValue::Str(v.into())
}

fn b(v: &[u8]) -> QueryValue {
    QueryValue::Bin(v.to_vec())
}

#[test]
fn base64_roundtrip_and_error() {
    let r = reg();
    assert_eq!(r.call("base64_enc", &[s("foo")]).unwrap(), s("Zm9v"));
    assert_eq!(r.call("base64_dec", &[s("Zm9v")]).unwrap(), b(b"foo"));
    // error: invalid base64 alphabet
    assert_eq!(
        r.call("base64_dec", &[s("!!!!")]).unwrap_err().code,
        "decode_failed"
    );
}

#[test]
fn base64url_roundtrip_and_error() {
    let r = reg();
    // 0xfb 0xff encodes to "-_" in url-safe (vs "+/" standard).
    let enc = r.call("base64url_enc", &[b(&[0xfb, 0xff])]).unwrap();
    assert_eq!(enc, s("-_8="));
    assert_eq!(r.call("base64url_dec", &[enc]).unwrap(), b(&[0xfb, 0xff]));
    assert_eq!(
        r.call("base64url_dec", &[s("****")]).unwrap_err().code,
        "decode_failed"
    );
}

#[test]
fn hex_roundtrip_and_error() {
    let r = reg();
    assert_eq!(r.call("hex_enc", &[b(&[0xde, 0xad])]).unwrap(), s("dead"));
    assert_eq!(r.call("hex_dec", &[s("dead")]).unwrap(), b(&[0xde, 0xad]));
    // error: odd length / non-hex digit
    assert_eq!(
        r.call("hex_dec", &[s("xyz")]).unwrap_err().code,
        "decode_failed"
    );
}

#[test]
fn base32_roundtrip_and_error() {
    let r = reg();
    assert_eq!(r.call("base32_enc", &[s("foo")]).unwrap(), s("MZXW6==="));
    assert_eq!(r.call("base32_dec", &[s("MZXW6===")]).unwrap(), b(b"foo"));
    // error: invalid base32 symbol
    assert_eq!(
        r.call("base32_dec", &[s("0189")]).unwrap_err().code,
        "decode_failed"
    );
}

#[test]
fn url_encode_decode_and_error() {
    let r = reg();
    assert_eq!(r.call("url_encode", &[s("a b/c")]).unwrap(), s("a%20b%2Fc"));
    assert_eq!(r.call("url_decode", &[s("a%20b%2Fc")]).unwrap(), s("a b/c"));
    // error: percent sequence decodes to invalid UTF-8 (lone 0xFF byte)
    assert_eq!(
        r.call("url_decode", &[s("%FF")]).unwrap_err().code,
        "decode_failed"
    );
}

#[test]
fn html_escape_metachars() {
    let r = reg();
    assert_eq!(
        r.call("html_escape", &[s("<a href=\"x\">&'</a>")]).unwrap(),
        s("&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;")
    );
    // edge: a plain string is unchanged
    assert_eq!(r.call("html_escape", &[s("plain")]).unwrap(), s("plain"));
}

#[test]
fn str_escape_chars_specials() {
    let r = reg();
    assert_eq!(
        r.call("json_escape", &[s("a\"\\\n\t")]).unwrap(),
        s("a\\\"\\\\\\n\\t")
    );
    // edge: control char below 0x20 uses \u escape
    assert_eq!(r.call("json_escape", &[s("\u{01}")]).unwrap(), s("\\u0001"));
}

#[test]
fn encoders_reject_non_text_non_bin() {
    let r = reg();
    assert_eq!(
        r.call("base64_enc", &[QueryValue::Int(7)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
    assert_eq!(
        r.call("hex_enc", &[QueryValue::Int(7)]).unwrap_err().code,
        "type_mismatch"
    );
}
