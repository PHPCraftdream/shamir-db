//! End-to-end coverage for typed errors returned by compiled guest functions.

use shamir_db::ShamirDb;
use shamir_engine::function::Params;

use super::lifecycle::compile_or_skip;

const USER_ERROR_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn user_error(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Err(Error::user("nope"))
}
"#;

const CODEC_ERROR_SRC: &str = r#"use shamir::prelude::*;
#[shamir::function]
pub async fn codec_error(_ctx: Ctx, _batch: Batch, _params: Params) -> Result<Value> {
    Err(Error::codec("bad bytes"))
}
"#;

#[tokio::test]
async fn rv_sdk_6d2497ca_guest_error_kinds_reach_host() {
    let db = ShamirDb::init_memory().await.expect("database");
    if !compile_or_skip(&db, "user_error", USER_ERROR_SRC).await {
        return;
    }
    if !compile_or_skip(&db, "codec_error", CODEC_ERROR_SRC).await {
        return;
    }
    let user = db
        .invoke_function("user_error", Params::new())
        .await
        .expect_err("user error must fail");
    assert!(user.to_string().contains("user error: nope"), "{user}");
    let codec = db
        .invoke_function("codec_error", Params::new())
        .await
        .expect_err("codec error must fail");
    let codec = codec.to_string();
    assert!(codec.contains("compute error"), "{codec}");
    assert!(codec.contains("guest codec: bad bytes"), "{codec}");
}
