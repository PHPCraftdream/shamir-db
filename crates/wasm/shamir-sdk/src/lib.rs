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

mod api;
mod error;
pub mod prelude;
mod runtime;

// Module-path aliases so that every pre-existing `shamir_sdk::<module>::…`
// and in-crate `crate::<module>::…` path keeps resolving after the move into
// `api/` and `runtime/`.
pub(crate) use api::{context, db, http, value};
pub use runtime::__rt;
pub use runtime::validation;
pub(crate) use runtime::{host_imports, params};

// Re-export the proc-macro at the crate root so that `#[shamir_sdk::function]`
// resolves, and with `use shamir_sdk as shamir;` so does `#[shamir::function]`.
pub use shamir_sdk_macros::function;
pub use shamir_sdk_macros::procedure;
pub use shamir_sdk_macros::scalar;
pub use shamir_sdk_macros::validator;

pub use validation::{IntoFieldPath, Validation, ValidationError};

pub use context::{Batch, Ctx};
pub use db::{Db, Table};
pub use error::{Error, ErrorKind, Result};
pub use http::{HttpRequest, HttpResponse};
pub use params::Params;
pub use value::Value;

/// The query builder (types + `q!`/`filter!` macros), available with the
/// `query-builder` feature, for use inside a `#[procedure]` via
/// [`Db::execute`](crate::Db::execute).
#[cfg(feature = "query-builder")]
pub use shamir_query_builder as builder;

/// Builder macros for use inside a `#[procedure]` (with the `query-builder`
/// feature): `q!`, `filter!`, `doc!`.
///
/// Note: `q!`/`filter!` are proc-macros that expand to absolute
/// `::shamir_query_builder::…` paths, so a guest using them must **also add
/// `shamir-query-builder` as a direct dependency** (re-exporting the names
/// here is not enough for path resolution). The builder-method API
/// ([`builder`]) works through `shamir-sdk` alone.
#[cfg(feature = "query-builder")]
pub use shamir_query_builder::{doc, filter, q};

#[cfg(test)]
mod tests;
