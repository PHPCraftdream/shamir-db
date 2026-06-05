use shamir_sdk::prelude::*;

#[shamir_sdk::function]
pub async fn identity(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
    let n = params.i64("n")?;
    Ok(Value::Int(n))
}
