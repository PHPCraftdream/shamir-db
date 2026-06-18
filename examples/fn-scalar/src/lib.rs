//! Example: a pure **scalar** function.
//!
//! A scalar takes `(params: Params) -> Result<Value>` and has **no** `Ctx` —
//! it cannot access the database, call other functions, or perform HTTP.
//! This makes it safe for use in filters, indexes, and computed columns.
//!
//! Build:
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown -p fn-scalar
//! ```

use shamir_sdk::prelude::*;

/// Uppercases a string parameter `"s"`.
///
/// ```text
/// { "s": "hello" }   →   "HELLO"
/// ```
#[shamir_sdk::scalar]
pub async fn upper(params: Params) -> Result<Value> {
    let s = params.str("s")?;
    Ok(Value::Str(s.to_uppercase()))
}
