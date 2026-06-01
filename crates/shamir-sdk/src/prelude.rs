//! Prelude for the ShamirDB guest function SDK.
//!
//! Import with `use shamir_sdk::prelude::*;`.
//!
//! To use the `#[shamir::function]` form, write:
//! ```ignore
//! use shamir_sdk as shamir;
//! use shamir::prelude::*;
//! ```
//!
//! The canonical form is `#[shamir_sdk::function]`:
//! ```ignore
//! use shamir_sdk::prelude::*;
//!
//! #[shamir_sdk::function]
//! pub async fn my_fn(ctx: Ctx, batch: Batch, params: Params) -> Result<Value> { ... }
//! ```

pub use crate::{Batch, Ctx, Db, Error, Params, Result, Table, Value};
pub use shamir_sdk_macros::function;
