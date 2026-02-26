//! Write operations module.
//!
//! Contains Insert, Update, Set (upsert), and Delete operations.

mod types;

pub use types::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect};

#[cfg(test)]
mod tests;
