//! Integration test: `#[scalar]` compiles with the correct 1-arg signature.
//!
//! This is an integration test (separate crate) so the `#[no_mangle]`
//! ABI symbols (`shamir_alloc`, `shamir_call`) don't collide with other
//! macro-expansion tests.

#[shamir_sdk::scalar]
pub async fn triple(params: shamir_sdk::Params) -> shamir_sdk::Result<shamir_sdk::Value> {
    let n: i64 = params.i64("n")?;
    Ok(shamir_sdk::Value::Int(n * 3))
}

#[test]
fn scalar_compiles() {
    // The macro expanded successfully — the test is the compilation itself.
}
