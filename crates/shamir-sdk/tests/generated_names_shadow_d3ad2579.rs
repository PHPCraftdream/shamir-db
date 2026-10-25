//! Regression test for RV-SDK-MACROS-D3AD2579: the generated code must use
//! fully qualified std names. This file deliberately shadows `Vec`,
//! `Option` and the `vec!` macro.
//!
//! Note on `Some`/`None`: the expansion uses fully qualified
//! `::core::option::Option::Some`/`::None`, which no crate-root item can
//! shadow (path-qualified resolution bypasses crate-root names). Genuine
//! shadowing of fully qualified variant paths is impossible in Rust, so
//! this fixture pins the shadow-able surface: the prelude type names
//! `Vec`/`Option` and the `vec!` macro. Any accidental bare `vec!` in the
//! generated code now hits a `compile_error!` trap and fails this fixture
//! hard.

// Type namespace shadows: the macro must not depend on these prelude names.
#[allow(dead_code)]
struct Vec;
#[allow(dead_code)]
enum Option {}

// Macro namespace shadow: any use of bare `vec!` in generated code must
// fail the fixture instead of silently resolving to a forwarding shim.
// (Deliberately unused here — hence the allow.)
#[allow(unused_macros)]
macro_rules! vec {
    ($($t:tt)*) => {
        compile_error!("generated code must not resolve to the shadowed vec!")
    };
}

#[shamir_sdk::validator]
pub async fn shadowed_prelude(
    _record: shamir_sdk::Value,
    _old_record: ::core::option::Option<shamir_sdk::Value>,
    _ctx: shamir_sdk::Ctx,
) -> shamir_sdk::Validation {
    shamir_sdk::Validation::accept()
}

#[test]
fn rv_sdk_macros_d3ad2579_shadowed_std_names_still_compile() {
    // The test body must not use `vec!` itself (it would hit the
    // compile_error! trap); compilation of this crate under the shadows is
    // the assertion.
    let _v: ::std::vec::Vec<u8> = ::std::vec::Vec::new();
}
