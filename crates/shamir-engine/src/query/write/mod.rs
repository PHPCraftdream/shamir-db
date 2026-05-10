//! Write operations module.
//!
//! Pure DTOs (InsertOp / UpdateOp / SetOp / DeleteOp / WriteResult) live
//! in `shamir-query-types::write`. Re-exported here so existing
//! `crate::query::write::*` paths inside the engine resolve unchanged.

pub use shamir_query_types::write::{
    DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect, WriteResult,
};

#[cfg(test)]
mod tests;
