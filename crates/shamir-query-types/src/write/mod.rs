//! Write-operation DTOs (Insert, Update, Set, Delete) + WriteResult.

pub mod types;
pub mod write_result;

pub use types::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect};
pub use write_result::WriteResult;
