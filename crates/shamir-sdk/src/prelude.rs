//! Prelude for the ShamirDB guest function SDK.
//!
//! Import with `use shamir_sdk::prelude::*;` — this single import covers
//! **all** function kinds. The prelude is intentionally flat (not split
//! per-kind) because the types are small and overlap heavily.
//!
//! # What each kind needs (all re-exported here)
//!
//! | Kind | Types used |
//! |------|-----------|
//! | `#[scalar]` | `Params`, `Value`, `Result`, `Error` |
//! | `#[procedure]` | `Ctx`, `Db`, `Table`, `Params`, `Value`, `Result`, `Error` |
//! | `#[function]` | `Ctx`, `Batch`, `Db`, `Table`, `Params`, `Value`, `Result`, `Error` |
//! | `#[validator]` | `Value`, `Ctx`, `Validation`, `ValidationError`, `IntoFieldPath` |
//!
//! HTTP types (`HttpRequest`, `HttpResponse`) are also exported for
//! procedures/functions that call `ctx.http_fetch()`.
//!
//! # Examples
//!
//! ```ignore
//! use shamir_sdk::prelude::*;
//!
//! #[shamir_sdk::scalar]
//! pub async fn upper(params: Params) -> Result<Value> {
//!     Ok(Value::Str(params.str("s")?.to_uppercase()))
//! }
//! ```
//!
//! ```ignore
//! use shamir_sdk::prelude::*;
//!
//! #[shamir_sdk::procedure]
//! pub async fn list_all(ctx: Ctx, params: Params) -> Result<Value> {
//!     let rows = ctx.db().table(params.str("table")?).query(None)?;
//!     Ok(Value::List(rows))
//! }
//! ```

pub use crate::{
    Batch, Ctx, Db, Error, HttpRequest, HttpResponse, IntoFieldPath, Params, Result, Table,
    Validation, ValidationError, Value,
};
pub use shamir_sdk_macros::function;
pub use shamir_sdk_macros::procedure;
pub use shamir_sdk_macros::scalar;
pub use shamir_sdk_macros::validator;
