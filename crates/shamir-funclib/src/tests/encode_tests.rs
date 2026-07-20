use crate::encode;
use crate::registry::ScalarRegistry;
use rust_decimal::Decimal;
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

// ============================================================================
// to_json / parse_json
// ============================================================================

fn map_of(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = shamir_types::types::common::new_map();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    QueryValue::Map(m)
}

#[test]
fn to_json_map_produces_valid_json() {
    let r = reg();
    let v = map_of(&[
        ("name", s("Alice")),
        ("age", QueryValue::Int(30)),
        ("active", QueryValue::Bool(true)),
        ("note", QueryValue::Null),
    ]);
    let json_str = match r.call("to_json", &[v]).unwrap() {
        QueryValue::Str(s) => s,
        other => panic!("expected Str, got {other:?}"),
    };
    // Parse with plain serde_json::Value to confirm it's well-formed JSON
    // (independent of QueryValue round-trip).
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("to_json output is valid JSON");
    assert_eq!(parsed["name"], serde_json::json!("Alice"));
    assert_eq!(parsed["age"], serde_json::json!(30));
    assert_eq!(parsed["active"], serde_json::json!(true));
    assert!(parsed["note"].is_null());
}

#[test]
fn parse_json_object_produces_map() {
    let r = reg();
    let result = r
        .call("parse_json", &[s(r#"{"x": 1, "y": "hello"}"#)])
        .unwrap();
    match result {
        QueryValue::Map(m) => {
            assert_eq!(m.get("x"), Some(&QueryValue::Int(1)));
            assert_eq!(m.get("y"), Some(&s("hello")));
        }
        other => panic!("expected Map, got {other:?}"),
    }
}

#[test]
fn parse_json_array_produces_list() {
    let r = reg();
    let result = r.call("parse_json", &[s(r#"[1, 2, 3]"#)]).unwrap();
    match result {
        QueryValue::List(l) => {
            assert_eq!(l.len(), 3);
            assert_eq!(l[0], QueryValue::Int(1));
            assert_eq!(l[1], QueryValue::Int(2));
            assert_eq!(l[2], QueryValue::Int(3));
        }
        other => panic!("expected List, got {other:?}"),
    }
}

#[test]
fn parse_json_malformed_returns_decode_failed() {
    let r = reg();
    assert_eq!(
        r.call("parse_json", &[s(r#"{"unbalanced""#)])
            .unwrap_err()
            .code,
        "decode_failed"
    );
    // Totally non-JSON text.
    assert_eq!(
        r.call("parse_json", &[s("not json at all")])
            .unwrap_err()
            .code,
        "decode_failed"
    );
}

#[test]
fn to_json_then_parse_json_roundtrips_safe_variants() {
    // Round-trip holds for Null/Bool/Int/F64/Str/List/Map (arbitrarily nested).
    // Dec/Big/Set/Bin are NOT round-trip-safe (they decay — see dedicated tests below).
    let r = reg();
    let v = map_of(&[
        ("n", QueryValue::Null),
        ("b", QueryValue::Bool(false)),
        ("i", QueryValue::Int(42)),
        ("f", QueryValue::F64(2.5)),
        ("s", s("hello")),
        (
            "l",
            QueryValue::List(vec![QueryValue::Int(1), s("two"), QueryValue::Bool(true)]),
        ),
        ("nested", map_of(&[("inner", QueryValue::Int(99))])),
    ]);
    let json_str = r.call("to_json", std::slice::from_ref(&v)).unwrap();
    let back = r.call("parse_json", &[json_str]).unwrap();
    assert_eq!(back, v, "round-trip must be lossless for safe variants");
}

#[test]
fn to_json_then_parse_json_decays_dec_to_str() {
    // INTENTIONAL decay: Dec serializes as a JSON string, and parse_json
    // always produces Str for JSON strings. This is the same type-fidelity
    // limitation as this codebase's msgpack round-trip — NOT a bug.
    let r = reg();
    let v = QueryValue::Dec(Decimal::from(42));
    let json_str = r.call("to_json", std::slice::from_ref(&v)).unwrap();
    let back = r.call("parse_json", &[json_str]).unwrap();
    assert_eq!(back, s("42"), "Dec decays to Str through JSON round-trip");
}

#[test]
fn to_json_then_parse_json_decays_big_to_str() {
    // INTENTIONAL decay: Big serializes as a JSON string.
    let r = reg();
    let v = QueryValue::Big(num_bigint::BigInt::from(999));
    let json_str = r.call("to_json", std::slice::from_ref(&v)).unwrap();
    let back = r.call("parse_json", &[json_str]).unwrap();
    assert_eq!(back, s("999"), "Big decays to Str through JSON round-trip");
}

#[test]
fn to_json_then_parse_json_decays_set_to_list() {
    // INTENTIONAL decay: Set serializes as a JSON array (same as List),
    // and parse_json always produces List for JSON arrays.
    let r = reg();
    let mut set = shamir_types::types::common::new_set::<QueryValue>();
    set.insert(QueryValue::Int(1));
    set.insert(QueryValue::Int(2));
    let v = QueryValue::Set(set);
    let json_str = r.call("to_json", std::slice::from_ref(&v)).unwrap();
    let back = r.call("parse_json", &[json_str]).unwrap();
    match back {
        QueryValue::List(l) => assert_eq!(l.len(), 2, "Set decays to List through JSON round-trip"),
        other => panic!("expected List (Set decay), got {other:?}"),
    }
}

#[test]
fn to_json_then_parse_json_decays_bin_to_list_of_ints() {
    // INTENTIONAL decay: Bin serializes as a JSON array of byte-value
    // numbers, and parse_json produces List of Ints for JSON arrays.
    let r = reg();
    let v = b(&[10, 20, 30]);
    let json_str = r.call("to_json", std::slice::from_ref(&v)).unwrap();
    let back = r.call("parse_json", &[json_str]).unwrap();
    match back {
        QueryValue::List(l) => {
            assert_eq!(l.len(), 3, "Bin decays to List of Ints");
            assert_eq!(l[0], QueryValue::Int(10));
            assert_eq!(l[1], QueryValue::Int(20));
            assert_eq!(l[2], QueryValue::Int(30));
        }
        other => panic!("expected List (Bin decay), got {other:?}"),
    }
}
