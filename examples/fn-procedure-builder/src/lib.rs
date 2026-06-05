//! Example: a procedure that builds a query with the query builder and runs
//! it via `ctx.db().execute()` (SDK Stage B2 — builder in the guest).
//!
//! Build:
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown -p fn-procedure-builder
//! ```

use shamir_sdk::builder::{batch::Batch, write, Query};
use shamir_sdk::prelude::*;
use shamir_sdk::{doc, filter, q};

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

/// Builds the same batch as `count_big` but via `q!`, `filter!` and `doc!`
/// macros re-exported from `shamir_sdk`. Not a WASM entry-point (only one
/// `#[procedure]` per cdylib), but compiled under wasm32 — proving macro
/// reachability through the SDK.
#[allow(dead_code)]
async fn count_big_macro(ctx: Ctx) -> Result<Value> {
    let mut b = Batch::new();
    b.id("q_macro");

    // q! — declarative read query.
    b.query("rows", q!(from items where n >= 2));

    // filter! — standalone filter, composed into a read query via the builder.
    let f = filter!(status == "active" && n > 0);
    b.query("filtered", Query::from("items").where_(f));

    // doc! — build an insert document.
    b.insert(
        "seed",
        write::insert("items")
            .row(doc! { "n" => 99, "label" => "from_macro" })
            .build(),
    );

    let resp = ctx.db().execute(&b)?;
    let n = resp
        .results
        .get("rows")
        .map(|r| r.records.len())
        .unwrap_or(0);
    Ok(Value::Int(n as i64))
}
