//! Example: a **procedure** with database access.
//!
//! A procedure takes `(ctx: Ctx, params: Params) -> Result<Value>` and has
//! full access to `ctx.db()` (read/write tables), `ctx.call()` (invoke other
//! functions), and `ctx.http_fetch()` (egress HTTP).
//!
//! Build:
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown -p fn-procedure
//! ```

use shamir_sdk::prelude::*;

/// Lists all rows from the table whose name is passed as `"table"` param.
///
/// ```text
/// { "table": "users" }   →   [ { "id": 1, ... }, ... ]
/// ```
#[shamir_sdk::procedure]
pub async fn list_all(ctx: Ctx, params: Params) -> Result<Value> {
    let table_name = params.str("table")?;
    let rows = ctx.db().table(table_name).query(None)?;
    Ok(Value::List(rows))
}
