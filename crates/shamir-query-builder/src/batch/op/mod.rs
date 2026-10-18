//! Batch-op construction: the `IntoBatchOp` trait, its repo-admin impls,
//! the `TryIntoBatchOp` fallible conversion, and `BuildError`.

pub mod build_error;
#[path = "into_batch_op/into_batch_op.rs"]
pub mod into_batch_op;
pub mod try_into_batch_op;
