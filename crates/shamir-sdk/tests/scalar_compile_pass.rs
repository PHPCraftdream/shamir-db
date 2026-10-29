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

#[cfg(not(target_arch = "wasm32"))]
fn shamir_alloc(_: i32) -> i32 {
    17
}
#[cfg(not(target_arch = "wasm32"))]
fn shamir_call(_: i32, _: i32) -> i64 {
    23
}

#[test]
fn scalar_compiles() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        assert_eq!(shamir_alloc(0), 17);
        assert_eq!(shamir_call(0, 0), 23);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _: extern "C" fn(i32) -> i32 = shamir_alloc;
        let _: unsafe extern "C" fn(i32, i32) -> i64 = shamir_call;
    }
}
