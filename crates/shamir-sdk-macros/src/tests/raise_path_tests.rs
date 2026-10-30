//! Regression oracle for deliberate errors in every generated guest ABI.

use crate::{expand_function, expand_procedure, expand_scalar, expand_validator};
use quote::ToTokens;

#[test]
fn rv_sdk_e3875787_guest_exports_are_wasm_only() {
    let sources = [
        "async fn v(r: shamir_sdk::Value, o: Option<shamir_sdk::Value>, c: shamir_sdk::Ctx) -> shamir_sdk::Validation { shamir_sdk::Validation::accept() }",
        "async fn f(c: shamir_sdk::Ctx, b: shamir_sdk::Batch, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "async fn p(c: shamir_sdk::Ctx, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "async fn s(p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
    ];
    let expansions = [
        expand_validator(
            syn::parse_str(sources[0]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_function(
            syn::parse_str(sources[1]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_procedure(
            syn::parse_str(sources[2]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_scalar(
            syn::parse_str(sources[3]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
    ];
    for expansion in expansions {
        let file: syn::File = syn::parse2(expansion).unwrap();
        for (name, expected) in [("shamir_alloc", 1), ("shamir_call", 2)] {
            let function = file
                .items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Fn(function) if function.sig.ident == name => Some(function),
                    _ => None,
                })
                .expect("guest export");
            assert!(function
                .attrs
                .iter()
                .any(|attr| attr.to_token_stream().to_string()
                    == "# [cfg (target_arch = \"wasm32\")]"));
            assert!(function
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("no_mangle")));
            assert_eq!(function.sig.inputs.len(), expected);
            for argument in &function.sig.inputs {
                match argument {
                    syn::FnArg::Typed(argument) => {
                        assert_eq!(
                            argument.ty.to_token_stream().to_string(),
                            "i32",
                            "{name} argument"
                        );
                    }
                    syn::FnArg::Receiver(_) => panic!("{name} has a receiver"),
                }
            }
            let return_type = match &function.sig.output {
                syn::ReturnType::Type(_, ty) => ty.to_token_stream().to_string(),
                syn::ReturnType::Default => panic!("{name} lacks a return type"),
            };
            assert_eq!(
                return_type,
                if name == "shamir_alloc" { "i32" } else { "i64" }
            );
            assert_eq!(
                function
                    .sig
                    .abi
                    .as_ref()
                    .unwrap()
                    .name
                    .as_ref()
                    .unwrap()
                    .value(),
                "C"
            );
        }
    }
}

#[test]
fn rv_sdk_macros_bb6378e2_err_arms_use_raise() {
    let sources = [
        "pub async fn v(r: shamir_sdk::Value, o: Option<shamir_sdk::Value>, c: shamir_sdk::Ctx) -> shamir_sdk::Validation { shamir_sdk::Validation::accept() }",
        "pub async fn f(c: shamir_sdk::Ctx, b: shamir_sdk::Batch, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "pub async fn p(c: shamir_sdk::Ctx, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "pub async fn s(p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
    ];
    let expansions = [
        expand_validator(
            syn::parse_str(sources[0]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_function(
            syn::parse_str(sources[1]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_procedure(
            syn::parse_str(sources[2]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
        expand_scalar(
            syn::parse_str(sources[3]).unwrap(),
            &syn::parse_quote!(::shamir_sdk),
        )
        .unwrap(),
    ];
    for expansion in expansions {
        let expansion = expansion.to_token_stream().to_string();
        assert!(expansion.contains("__rt :: raise"), "{expansion}");
        assert!(!expansion.contains("trap (& e . to_string"), "{expansion}");
    }
}
