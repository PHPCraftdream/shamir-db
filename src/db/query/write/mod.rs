//! Write operations module.
//!
//! Contains Insert, Update, Set (upsert), and Delete operations.

mod types;
mod write_result;

pub use types::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect};
pub use write_result::WriteResult;

#[cfg(test)]
mod tests;
