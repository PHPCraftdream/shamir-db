//! Regression test for RV-SDK-MACROS-853FC350: `#[validator]` must accept the
//! rooted `::shamir_sdk::Validation` return spelling (structural `syn::Type`
//! matching). On the old string-substitution code rooted spellings were
//! rejected.

use ::core::option::Option;

#[shamir_sdk::validator]
pub async fn qualified_validation(
    _record: shamir_sdk::Value,
    _old_record: Option<shamir_sdk::Value>,
    _ctx: shamir_sdk::Ctx,
) -> ::shamir_sdk::Validation {
    shamir_sdk::Validation::accept()
}

#[test]
fn rv_sdk_macros_853fc350_qualified_validation_spelling_compiles() {
    // Compilation of this crate is the assertion.
}
