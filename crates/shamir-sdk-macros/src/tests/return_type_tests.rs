//! Structural return-type matcher tests + `expand_validator` oracle.

use crate::{expand_validator, is_result_value_return, is_validation_return};

fn rt(src: &str) -> bool {
    is_result_value_return(&syn::parse_str(src).unwrap())
}

fn vr(src: &str) -> bool {
    is_validation_return(&syn::parse_str(src).unwrap())
}

#[test]
fn rv_sdk_macros_853fc350_result_value_accepts_all_rooted_and_bare_spellings() {
    assert!(rt("::std::result::Result<Value, shamir_sdk::Error>"));
    assert!(rt("std::result::Result<Value, Error>"));
    assert!(rt("core::result::Result<Value, Error>"));
    assert!(rt("Result<Value>"));
    assert!(rt("Result<Value, Error>"));
    assert!(rt("shamir_sdk::Result<shamir_sdk::Value>"));
    assert!(rt("crate::Result<Value>"));
}

#[test]
fn rv_sdk_macros_853fc350_result_value_rejects_mismatched_types() {
    assert!(!rt("std::result::Result<String, Error>"));
    assert!(!rt("Result"));
    assert!(!rt("Result<Value, Error, Extra>"));
    assert!(!rt("Option<Value>"));
    assert!(!rt("Validation<T>"));
}

#[test]
fn rv_sdk_macros_853fc350_validation_accepts_any_prefix() {
    assert!(vr("::shamir_sdk::Validation"));
    assert!(vr("shamir_sdk::Validation"));
    assert!(vr("Validation"));
    assert!(vr("crate::Validation"));
    // Any prefix is structurally accepted; the compiler is the real guard.
    assert!(vr("mycrate::Validation"));
}

#[test]
fn rv_sdk_macros_853fc350_validation_rejects_generified_and_other_types() {
    assert!(!vr("Validation<T>"));
    assert!(!vr("Result<Value>"));
    assert!(!vr("Option<Validation>"));
}

/// Minimal validator source; the expansion asserts on its structure.
const VALIDATOR_SRC: &str = "\
pub async fn v(_r: shamir_sdk::Value, _o: Option<shamir_sdk::Value>, _c: shamir_sdk::Ctx) \
-> shamir_sdk::Validation { shamir_sdk::Validation::accept() }";

#[test]
fn rv_sdk_macros_4db0d871_expansion_traps_on_decode_and_missing_record() {
    let fn_item = syn::parse_str::<syn::ItemFn>(VALIDATOR_SRC).unwrap();
    let s = expand_validator(fn_item).to_string();
    // Decode failure must trap: the decode match arm's Err branch must call
    // `trap` before the `record` extraction happens.
    let dec = s
        .find("decode_params")
        .expect("decode_params call in expansion");
    let rec = s
        .find("params . get (\"record\")")
        .expect("record extraction");
    let decode_arm = &s[dec..rec];
    assert!(
        decode_arm.contains("trap") && decode_arm.contains(":: Err"),
        "decode match arm's Err branch must call trap; got: {decode_arm}"
    );
    // Missing `record` must trap: the expansion emits the message as ONE
    // string literal `"validator: missing \`record\` parameter"`, and
    // TokenStream::to_string does not add spaces inside a literal.
    assert!(s.contains("validator: missing"), "got: {s}");
    // The `record` arm must not fall back to `Value::Null`.
    let old = s[rec..]
        .find("params . get (\"old_record\")")
        .expect("old_record extraction");
    assert!(
        !s[rec..rec + old].contains("shamir_sdk :: Value :: Null"),
        "record arm must trap, not fall back to Null; got: {}",
        &s[rec..rec + old]
    );
}
