//! Structural return-type matcher tests + `expand_validator` oracle.

use crate::{
    expand_function, expand_procedure, expand_scalar, expand_validator, is_result_value_return,
    is_validation_return,
};
use quote::ToTokens;

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

#[test]
fn rv_sdk_macros_966aa3ad_raw_function_identifier_builds_valid_helper_name() {
    let cases = [
        (
            "validator",
            "pub async fn r#type(r: shamir_sdk::Value, o: Option<shamir_sdk::Value>, c: shamir_sdk::Ctx) -> shamir_sdk::Validation { shamir_sdk::Validation::accept() }",
        ),
        (
            "function",
            "pub async fn r#type(c: shamir_sdk::Ctx, b: shamir_sdk::Batch, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        ),
        (
            "procedure",
            "pub async fn r#type(c: shamir_sdk::Ctx, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        ),
        (
            "scalar",
            "pub async fn r#type(p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        ),
    ];
    for (kind, source) in cases {
        let fn_item = syn::parse_str::<syn::ItemFn>(source).unwrap();
        let expansion = match kind {
            "validator" => expand_validator(fn_item).unwrap(),
            "function" => expand_function(fn_item).unwrap(),
            "procedure" => expand_procedure(fn_item).unwrap(),
            _ => expand_scalar(fn_item).unwrap(),
        }
        .to_string();
        assert!(
            expansion.contains("__shamir_impl_type"),
            "{kind}: {expansion}"
        );
        assert!(
            !expansion.contains("__shamir_impl_r # type"),
            "{kind}: {expansion}"
        );
    }
}

/// Minimal validator source; the expansion asserts on its structure.
const VALIDATOR_SRC: &str = "\
pub async fn v(_r: shamir_sdk::Value, _o: Option<shamir_sdk::Value>, _c: shamir_sdk::Ctx) \
-> shamir_sdk::Validation { shamir_sdk::Validation::accept() }";

#[test]
fn rv_sdk_macros_4db0d871_expansion_traps_on_decode_and_missing_record() {
    let fn_item = syn::parse_str::<syn::ItemFn>(VALIDATOR_SRC).unwrap();
    let s = expand_validator(fn_item).unwrap().to_string();
    // Decode failure must raise: the decode match arm's Err branch must call
    // `raise` before the `record` extraction happens.
    let dec = s
        .find("decode_params")
        .expect("decode_params call in expansion");
    let rec = s.find("key == \"record\"").expect("record extraction");
    let decode_arm = &s[dec..rec];
    assert!(
        decode_arm.contains("raise") && decode_arm.contains(":: Err"),
        "decode match arm's Err branch must call raise; got: {decode_arm}"
    );
    // Missing `record` must raise: the expansion emits the message as ONE
    // string literal "validator: missing `record` parameter", and
    // TokenStream::to_string does not add spaces inside a literal.
    assert!(s.contains("validator: missing"), "got: {s}");
    // The `record` arm must not fall back to `Value::Null`.
    let old = s[rec..]
        .find("key == \"old_record\"")
        .expect("old_record extraction");
    assert!(
        !s[rec..rec + old].contains("shamir_sdk :: Value :: Null"),
        "record arm must raise, not fall back to Null; got: {}",
        &s[rec..rec + old]
    );
}

#[test]
fn rv_sdk_macros_2638faff_validator_moves_values_and_drops_payload_before_await() {
    let fn_item = syn::parse_str::<syn::ItemFn>(VALIDATOR_SRC).unwrap();
    let expansion = expand_validator(fn_item).unwrap().to_string();
    assert!(
        expansion.contains("params . into_raw ()"),
        "got: {expansion}"
    );
    assert!(
        expansion.contains("entries . remove"),
        "values must be removed without reordering remaining duplicate keys; got: {expansion}"
    );
    assert!(!expansion.contains("swap_remove"), "got: {expansion}");
    assert!(expansion.contains("drop (entries)"), "got: {expansion}");
    assert!(!expansion.contains("v . clone ()"), "got: {expansion}");
    let drop_payload = expansion.find("drop (entries)").expect("payload drop");
    let await_call = expansion.find("block_on").expect("validation await");
    assert!(
        drop_payload < await_call,
        "payload must drop before await: {expansion}"
    );
}

#[test]
fn rv_sdk_macros_fb968c81_all_emitters_return_spanned_signature_errors() {
    let cases = [
        (
            "#[validator]",
            "pub fn v(r: Value, o: Option<Value>, c: Ctx) -> Validation { todo!() }",
            "requires an async function",
        ),
        (
            "#[function]",
            "pub async fn f(c: Ctx, b: Batch) -> Result<Value> { todo!() }",
            "expects exactly 3 arguments",
        ),
        (
            "#[procedure]",
            "pub async fn p(c: Ctx, p: Params) -> String { todo!() }",
            "invalid return type",
        ),
        (
            "#[scalar]",
            "pub async fn s(c: Ctx) -> String { todo!() }",
            "has an invalid return type",
        ),
    ];
    for (kind, source, expected) in cases {
        let item = syn::parse_str::<syn::ItemFn>(source).unwrap();
        let error = match kind {
            "#[validator]" => expand_validator(item).unwrap_err(),
            "#[function]" => expand_function(item).unwrap_err(),
            "#[procedure]" => expand_procedure(item).unwrap_err(),
            _ => expand_scalar(item).unwrap_err(),
        };
        assert!(error.to_string().contains(expected), "{kind}: {error}");
        assert!(
            error
                .to_compile_error()
                .to_string()
                .contains("compile_error"),
            "{kind}: expected compile_error tokens"
        );
    }

    let scalar_ctx =
        syn::parse_str::<syn::ItemFn>("pub async fn s(c: Ctx) -> Result<Value> { todo!() }")
            .unwrap();
    let error = expand_scalar(scalar_ctx).unwrap_err();
    assert!(error.to_string().contains("must not take Ctx"));
}

#[test]
fn rv_sdk_macros_0a76d271_all_emitters_have_unsafe_contract_and_guards() {
    let cases = [
        ("validator", VALIDATOR_SRC),
        ("function", "pub async fn f(_c: shamir_sdk::Ctx, _b: shamir_sdk::Batch, _p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }"),
        ("procedure", "pub async fn p(_c: shamir_sdk::Ctx, _p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }"),
        ("scalar", "pub async fn s(_p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }"),
    ];
    for (kind, source) in cases {
        let item = syn::parse_str::<syn::ItemFn>(source).unwrap();
        let expansion = match kind {
            "validator" => expand_validator(item).unwrap(),
            "function" => expand_function(item).unwrap(),
            "procedure" => expand_procedure(item).unwrap(),
            _ => expand_scalar(item).unwrap(),
        };
        let file = syn::parse2::<syn::File>(expansion).unwrap();
        let entry = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(item) if item.sig.ident == "shamir_call" => Some(item),
                _ => None,
            })
            .expect("shamir_call expansion");
        assert!(entry.sig.unsafety.is_some(), "{kind}: must be unsafe");
        assert!(
            matches!(&entry.sig.abi, Some(syn::Abi { name: Some(name), .. }) if name.value() == "C"),
            "{kind}: must use C ABI"
        );
        let body = entry.block.to_token_stream().to_string();
        let guard = body.find("len < 0").expect("negative-length guard");
        let null_guard = body
            .find("len > 0 && ptr == 0")
            .expect("null-pointer guard");
        let slice = body.find("from_raw_parts").expect("raw slice conversion");
        assert!(
            guard < slice && null_guard < slice,
            "{kind}: both guards must precede from_raw_parts"
        );
        assert!(
            entry.attrs.iter().any(|attr| attr.path().is_ident("doc")
                && attr.meta.to_token_stream().to_string().contains("Safety")),
            "{kind}: safety documentation required"
        );
    }
}

#[test]
fn rv_sdk_macros_9ac2b50c_forward_patterns_as_hygienic_arguments() {
    let cases = [
        (
            "validator",
            "pub async fn v(mut x: Value, ref y: Option<Value>, _: Ctx) -> Validation { Validation::accept() }",
            3,
        ),
        (
            "function",
            "pub async fn f(mut x: Ctx, ref y: Batch, _: Params) -> Result<Value> { Ok(Value::Null) }",
            3,
        ),
        (
            "procedure",
            "pub async fn p(mut x: Ctx, ref y: Params) -> Result<Value> { Ok(Value::Null) }",
            2,
        ),
        (
            "scalar",
            "pub async fn s(mut x: Params) -> Result<Value> { Ok(Value::Null) }",
            1,
        ),
    ];
    for (kind, source, arity) in cases {
        let item = syn::parse_str::<syn::ItemFn>(source).unwrap();
        let original_sig = item.sig.clone();
        let expansion = match kind {
            "validator" => expand_validator(item).unwrap(),
            "function" => expand_function(item).unwrap(),
            "procedure" => expand_procedure(item).unwrap(),
            _ => expand_scalar(item).unwrap(),
        };
        let parsed = syn::parse2::<syn::File>(expansion.clone())
            .unwrap_or_else(|error| panic!("{kind}: {error}\n{expansion}"));
        let wrapper = parsed
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Fn(item)
                    if item.sig.ident == format!("__shamir_impl_{}", original_sig.ident) =>
                {
                    Some(item)
                }
                _ => None,
            })
            .expect("generated wrapper function");
        let nested_original = wrapper
            .block
            .stmts
            .iter()
            .find_map(|stmt| match stmt {
                syn::Stmt::Item(syn::Item::Fn(item)) if item.sig.ident == original_sig.ident => {
                    Some(item)
                }
                _ => None,
            })
            .expect("nested original user function");
        assert_eq!(
            nested_original.sig.to_token_stream().to_string(),
            original_sig.to_token_stream().to_string(),
            "original user signature changed: {kind}"
        );
        let rendered = expansion.to_string();
        let call_at = match kind {
            "validator" => rendered.rfind("v (__shamir_arg_0"),
            "function" => rendered.rfind("f (__shamir_arg_0"),
            "procedure" => rendered.rfind("p (__shamir_arg_0"),
            _ => rendered.rfind("s (__shamir_arg_0"),
        }
        .expect("generated user call");
        let call = &rendered[call_at..];
        let actual_call = &call[..call.find(" . await").unwrap_or(call.len())];
        for index in 0..arity {
            assert!(
                actual_call.contains(&format!("__shamir_arg_{index}")),
                "wrapper must pass each generated identifier directly: {kind}: {actual_call}"
            );
        }
        assert!(
            !actual_call.contains("mut __shamir_arg") && !actual_call.contains("ref __shamir_arg"),
            "patterns must not be call-position expressions: {kind}: {actual_call}"
        );
        assert!(
            rendered.contains("mut x"),
            "user function must retain mut x: {kind}: {rendered}"
        );
        if kind != "scalar" {
            assert!(
                rendered.contains("ref y"),
                "user function must retain ref y: {kind}: {rendered}"
            );
        }
    }
}
