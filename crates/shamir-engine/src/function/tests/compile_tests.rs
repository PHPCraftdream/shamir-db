use crate::function::{
    compile_rust_source, FnBatch, FnCtx, FunctionError, Params, ShamirFunction, WasmEngine,
    WasmFunction, WasmLimits,
};
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

const DOUBLE_SOURCE: &str = r#"
use shamir::prelude::*;

#[shamir::function]
pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n: i64 = params.i64("n")?;
    Ok(Value::Int(n * 2))
}
"#;

#[tokio::test]
async fn compile_and_invoke_double() {
    let wasm = match compile_rust_source(DOUBLE_SOURCE) {
        Ok(w) => w,
        Err(FunctionError::ToolchainUnavailable(msg)) => {
            eprintln!("SKIP compile_and_invoke_double: {msg}");
            return;
        }
        Err(e) => panic!("compile failed: {e}"),
    };

    let engine = Arc::new(WasmEngine::new().unwrap());
    let wf = WasmFunction::from_binary(engine, &wasm, WasmLimits::default()).unwrap();

    let mut params = Params::new();
    params.set("n", QueryValue::Int(21));

    let result = wf.call(&FnCtx::new(), &FnBatch::new(), &params).await;
    let val = result.expect("function should succeed");
    assert_eq!(val, QueryValue::Int(42), "double(21) should return 42");
}
