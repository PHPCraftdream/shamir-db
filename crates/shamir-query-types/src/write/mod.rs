//! Write-operation DTOs (Insert, Update, Set, Delete) + WriteResult.

pub mod inserted_record;
pub mod types;
pub mod write_result;

pub use inserted_record::InsertedRecord;
pub use types::{DeleteOp, InsertOp, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect};
pub use write_result::WriteResult;
