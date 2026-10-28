//! Regression oracle for deliberate errors in every generated guest ABI.

use crate::{expand_function, expand_procedure, expand_scalar, expand_validator};
use quote::ToTokens;

#[test]
fn rv_sdk_macros_bb6378e2_err_arms_use_raise() {
    let sources = [
        "pub async fn v(r: shamir_sdk::Value, o: Option<shamir_sdk::Value>, c: shamir_sdk::Ctx) -> shamir_sdk::Validation { shamir_sdk::Validation::accept() }",
        "pub async fn f(c: shamir_sdk::Ctx, b: shamir_sdk::Batch, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "pub async fn p(c: shamir_sdk::Ctx, p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
        "pub async fn s(p: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> { Ok(shamir_sdk::Value::Null) }",
    ];
    let expansions = [
        expand_validator(syn::parse_str(sources[0]).unwrap()).unwrap(),
        expand_function(syn::parse_str(sources[1]).unwrap()).unwrap(),
        expand_procedure(syn::parse_str(sources[2]).unwrap()).unwrap(),
        expand_scalar(syn::parse_str(sources[3]).unwrap()).unwrap(),
    ];
    for expansion in expansions {
        let expansion = expansion.to_token_stream().to_string();
        assert!(expansion.contains("__rt :: raise"), "{expansion}");
        assert!(!expansion.contains("trap (& e . to_string"), "{expansion}");
    }
}
