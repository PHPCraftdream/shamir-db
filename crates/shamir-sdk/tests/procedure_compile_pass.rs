//! Integration test: `#[procedure]` compiles with the correct 2-arg signature.
//!
//! This is an integration test (separate crate) so the `#[no_mangle]`
//! ABI symbols (`shamir_alloc`, `shamir_call`) don't collide with other
//! macro-expansion tests.

#[shamir_sdk::procedure]
pub async fn get_users(
    _ctx: shamir_sdk::Ctx,
    params: shamir_sdk::Params,
) -> shamir_sdk::Result<shamir_sdk::Value> {
    let _limit: i64 = params.i64("limit").unwrap_or(10);
    // In a real procedure: _ctx.db().table("users").query(None)
    Ok(shamir_sdk::Value::Null)
}

#[test]
fn procedure_compiles() {
    // The macro expanded successfully — the test is the compilation itself.
}
