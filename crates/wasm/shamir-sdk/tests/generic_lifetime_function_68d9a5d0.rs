//! Regression test for RV-SDK-MACROS-68D9A5D0: the expansion emits a generic
//! inner function (no impl block), so lifetime-only generics pass through and
//! compile. Pins the refutation of the E0207 premise.

use shamir_sdk::{Batch, Ctx, Params, Result, Value};

#[shamir_sdk::function]
pub async fn lifetime_generic<'a>(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    let _: &'a () = &();
    Ok(Value::Null)
}

#[test]
fn rv_sdk_macros_68d9a5d0_lifetime_generic_function_compiles() {
    // Compilation of this crate is the assertion.
}
