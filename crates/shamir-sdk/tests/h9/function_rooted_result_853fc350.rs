//! Regression test for RV-SDK-MACROS-853FC350: rooted Result spelling.

use shamir_sdk::{Batch, Ctx, Params, Value};

#[shamir_sdk::function]
pub async fn std_qualified_return(
    _ctx: Ctx,
    _batch: Batch,
    _params: Params,
) -> ::std::result::Result<Value, shamir_sdk::Error> {
    Ok(Value::Null)
}

#[test]
fn rv_sdk_macros_853fc350_std_result_spelling_compiles() {}
