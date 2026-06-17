use crate::registry::{v_bool, v_int, v_list, v_str, ScalarRegistry};
use crate::strings;
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    strings::register(&mut r);
    r
}

fn s(x: &str) -> QueryValue {
    QueryValue::Str(x.to_string())
}

#[test]
fn lower_upper() {
    let r = reg();
    assert_eq!(r.call("lower", &[s("AbC")]).unwrap(), v_str("abc".into()));
    assert_eq!(r.call("upper", &[s("AbC")]).unwrap(), v_str("ABC".into()));
    // error: wrong type
    assert_eq!(
        r.call("lower", &[QueryValue::Int(1)]).unwrap_err().code,
        "type_mismatch"
    );
    assert_eq!(
        r.call("upper", &[QueryValue::Int(1)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn trim_family() {
    let r = reg();
    assert_eq!(r.call("trim", &[s("  hi  ")]).unwrap(), v_str("hi".into()));
    assert_eq!(
        r.call("ltrim", &[s("  hi  ")]).unwrap(),
        v_str("hi  ".into())
    );
    assert_eq!(
        r.call("rtrim", &[s("  hi  ")]).unwrap(),
        v_str("  hi".into())
    );
    // arity: trim takes exactly one arg
    assert_eq!(r.call("trim", &[]).unwrap_err().code, "arity");
}

#[test]
fn length_and_byte_length() {
    let r = reg();
    // "é" is one char but two UTF-8 bytes.
    assert_eq!(r.call("length", &[s("é")]).unwrap(), v_int(1));
    assert_eq!(r.call("byte_length", &[s("é")]).unwrap(), v_int(2));
    // error: wrong type
    assert_eq!(
        r.call("length", &[QueryValue::Bool(true)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn substring_ok_and_err() {
    let r = reg();
    assert_eq!(
        r.call(
            "substring",
            &[s("hello"), QueryValue::Int(1), QueryValue::Int(3)]
        )
        .unwrap(),
        v_str("ell".into())
    );
    // start past end -> empty string (not an error)
    assert_eq!(
        r.call(
            "substring",
            &[s("hi"), QueryValue::Int(9), QueryValue::Int(3)]
        )
        .unwrap(),
        v_str("".into())
    );
    // negative start -> out_of_range
    assert_eq!(
        r.call(
            "substring",
            &[s("hi"), QueryValue::Int(-1), QueryValue::Int(1)]
        )
        .unwrap_err()
        .code,
        "out_of_range"
    );
}

#[test]
fn concat_nary() {
    let r = reg();
    assert_eq!(
        r.call("concat", &[s("a"), s("b"), s("c")]).unwrap(),
        v_str("abc".into())
    );
    // error: non-string arg
    assert_eq!(
        r.call("concat", &[s("a"), QueryValue::Int(2)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn replace_ok() {
    let r = reg();
    assert_eq!(
        r.call("replace", &[s("a-b-c"), s("-"), s("_")]).unwrap(),
        v_str("a_b_c".into())
    );
    // no occurrence -> unchanged
    assert_eq!(
        r.call("replace", &[s("abc"), s("z"), s("_")]).unwrap(),
        v_str("abc".into())
    );
}

#[test]
fn split_ok_and_empty_sep() {
    let r = reg();
    assert_eq!(
        r.call("split", &[s("a,b,c"), s(",")]).unwrap(),
        v_list(vec![s("a"), s("b"), s("c")])
    );
    // empty separator -> empty
    assert_eq!(
        r.call("split", &[s("abc"), s("")]).unwrap_err().code,
        "empty"
    );
}

#[test]
fn starts_ends_contains() {
    let r = reg();
    assert_eq!(
        r.call("starts_with", &[s("hello"), s("he")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("ends_with", &[s("hello"), s("lo")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("contains", &[s("hello"), s("ell")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("contains", &[s("hello"), s("zz")]).unwrap(),
        v_bool(false)
    );
}

#[test]
fn index_of_found_and_missing() {
    let r = reg();
    // char index, accounting for a multibyte prefix
    assert_eq!(r.call("index_of", &[s("éxy"), s("y")]).unwrap(), v_int(2));
    assert_eq!(r.call("index_of", &[s("abc"), s("z")]).unwrap(), v_int(-1));
}

#[test]
fn repeat_ok_and_neg() {
    let r = reg();
    assert_eq!(
        r.call("repeat", &[s("ab"), QueryValue::Int(3)]).unwrap(),
        v_str("ababab".into())
    );
    assert_eq!(
        r.call("repeat", &[s("ab"), QueryValue::Int(-1)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn reverse_ok() {
    let r = reg();
    assert_eq!(r.call("reverse", &[s("abc")]).unwrap(), v_str("cba".into()));
    // unicode-scalar reversal
    assert_eq!(r.call("reverse", &[s("áb")]).unwrap(), v_str("bá".into()));
}

#[test]
fn pad_left_right_and_err() {
    let r = reg();
    assert_eq!(
        r.call("pad_left", &[s("7"), QueryValue::Int(3), s("0")])
            .unwrap(),
        v_str("007".into())
    );
    assert_eq!(
        r.call("pad_right", &[s("7"), QueryValue::Int(3), s("0")])
            .unwrap(),
        v_str("700".into())
    );
    // already wide enough -> unchanged
    assert_eq!(
        r.call("pad_left", &[s("abcd"), QueryValue::Int(2), s("0")])
            .unwrap(),
        v_str("abcd".into())
    );
    // multi-char pad -> bad_pad
    assert_eq!(
        r.call("pad_left", &[s("7"), QueryValue::Int(3), s("ab")])
            .unwrap_err()
            .code,
        "bad_pad"
    );
    // negative length -> out_of_range
    assert_eq!(
        r.call("pad_right", &[s("7"), QueryValue::Int(-1), s("0")])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn is_reg_match_and_bad_regex() {
    let r = reg();
    assert_eq!(
        r.call("is_reg_match", &[s("abc123"), s(r"\d+")]).unwrap(),
        v_bool(true)
    );
    assert_eq!(
        r.call("is_reg_match", &[s("abc"), s(r"\d+")]).unwrap(),
        v_bool(false)
    );
    // invalid pattern -> bad_regex
    assert_eq!(
        r.call("is_reg_match", &[s("abc"), s("(")])
            .unwrap_err()
            .code,
        "bad_regex"
    );
}

#[test]
fn reg_query_and_no_match() {
    let r = reg();
    assert_eq!(
        r.call("reg_query", &[s("id=42;x"), s(r"\d+")]).unwrap(),
        v_str("42".into())
    );
    assert_eq!(
        r.call("reg_query", &[s("abc"), s(r"\d+")])
            .unwrap_err()
            .code,
        "no_match"
    );
}

#[test]
fn reg_query_all_ok() {
    let r = reg();
    assert_eq!(
        r.call("reg_query_all", &[s("a1b22c333"), s(r"\d+")])
            .unwrap(),
        v_list(vec![s("1"), s("22"), s("333")])
    );
    // no matches -> empty list
    assert_eq!(
        r.call("reg_query_all", &[s("abc"), s(r"\d+")]).unwrap(),
        v_list(vec![])
    );
}

#[test]
fn reg_captures_groups_and_no_match() {
    let r = reg();
    assert_eq!(
        r.call("reg_captures", &[s("2024-06"), s(r"(\d+)-(\d+)")])
            .unwrap(),
        v_list(vec![s("2024-06"), s("2024"), s("06")])
    );
    assert_eq!(
        r.call("reg_captures", &[s("xx"), s(r"(\d+)")])
            .unwrap_err()
            .code,
        "no_match"
    );
}

#[test]
fn reg_replace_ok() {
    let r = reg();
    assert_eq!(
        r.call("reg_replace", &[s("a1b2"), s(r"\d"), s("#")])
            .unwrap(),
        v_str("a#b#".into())
    );
    // bad pattern -> bad_regex
    assert_eq!(
        r.call("reg_replace", &[s("a"), s("["), s("#")])
            .unwrap_err()
            .code,
        "bad_regex"
    );
}

#[test]
fn reg_split_ok() {
    let r = reg();
    assert_eq!(
        r.call("reg_split", &[s("a1b22c"), s(r"\d+")]).unwrap(),
        v_list(vec![s("a"), s("b"), s("c")])
    );
}

#[test]
fn reg_count_ok() {
    let r = reg();
    assert_eq!(
        r.call("reg_count", &[s("a1b22c333"), s(r"\d+")]).unwrap(),
        v_int(3)
    );
    assert_eq!(
        r.call("reg_count", &[s("abc"), s(r"\d+")]).unwrap(),
        v_int(0)
    );
}

#[test]
fn reg_find_index_found_and_missing() {
    let r = reg();
    // char index of first match start, after a multibyte prefix
    assert_eq!(
        r.call("reg_find_index", &[s("é12"), s(r"\d+")]).unwrap(),
        v_int(1)
    );
    assert_eq!(
        r.call("reg_find_index", &[s("abc"), s(r"\d+")]).unwrap(),
        v_int(-1)
    );
}
