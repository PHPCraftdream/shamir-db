//! Write-operation DTOs (Insert, Update, Set, Delete) + WriteResult.

pub mod inserted_record;
pub mod types;
pub mod write_result;

#[cfg(test)]
mod tests;

pub use inserted_record::InsertedRecord;
pub use types::{
    DeleteOp, DeleteSelect, InsertOp, InsertSelect, SetOp, UpdateOp, UpdateReturnMode, UpdateSelect,
};
pub use write_result::WriteResult;
