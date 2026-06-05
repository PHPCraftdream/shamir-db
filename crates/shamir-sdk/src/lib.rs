//! Guest authoring SDK for ShamirDB user-defined functions.
//!
//! Function authors write plain async Rust and apply `#[shamir_sdk::function]`:
//!
//! ```ignore
//! use shamir_sdk::prelude::*;
//!
//! #[shamir_sdk::function]
//! pub async fn double(_ctx: Ctx, _batch: Batch, params: Params) -> Result<Value> {
//!     let n: i64 = params.i64("n")?;
//!     Ok(Value::Int(n * 2))
//! }
//! ```
//!
//! The macro hides the WASM ABI entirely. This crate also works on the host
//! target for testing.

pub mod __rt;
mod context;
mod db;
mod error;
mod host_imports;
mod http;
mod params;
pub mod prelude;
pub mod validation;
mod value;

// Re-export the proc-macro at the crate root so that `#[shamir_sdk::function]`
// resolves, and with `use shamir_sdk as shamir;` so does `#[shamir::function]`.
pub use shamir_sdk_macros::function;
pub use shamir_sdk_macros::procedure;
pub use shamir_sdk_macros::scalar;
pub use shamir_sdk_macros::validator;

pub use validation::{IntoFieldPath, Validation, ValidationError};

pub use context::{Batch, Ctx};
pub use db::{Db, Table};
pub use error::{Error, Result};
pub use http::{HttpRequest, HttpResponse};
pub use params::Params;
pub use value::Value;

#[cfg(test)]
mod tests;
