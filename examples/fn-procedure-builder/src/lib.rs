//! Example: a procedure that builds a query with the query builder and runs
//! it via `ctx.db().execute()` (SDK Stage B2 — builder in the guest).
//!
//! Build:
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown -p fn-procedure-builder
//! ```

use shamir_sdk::builder::{batch::Batch, Query};
use shamir_sdk::prelude::*;

/// Counts rows in `items` where `n >= 2`, built with the query builder and
/// executed through `ctx.db().execute()`.
#[shamir_sdk::procedure]
pub async fn count_big(ctx: Ctx, _params: Params) -> Result<Value> {
    let mut b = Batch::new();
    b.id("q");
    b.query("rows", Query::from("items").where_gte("n", 2_i64));
    let resp = ctx.db().execute(&b)?;
    let n = resp
        .results
        .get("rows")
        .map(|r| r.records.len())
        .unwrap_or(0);
    Ok(Value::Int(n as i64))
}
