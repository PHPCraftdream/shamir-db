//! Batch execution pipeline: planning/validation, the stage executor,
//! execution deadlines, executor traits and interactive transactions.

pub(super) mod batch_execute;
pub(super) mod batch_validate;
pub(crate) mod exec_scope;
pub(super) mod execution_deadline;
pub(super) mod executor_traits;
pub(super) mod interactive_tx;
