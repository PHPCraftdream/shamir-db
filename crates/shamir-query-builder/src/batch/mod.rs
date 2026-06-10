//! Batch assembler + typed `Handle`/`RowRef` dependency references.
//!
//! `Batch` accumulates queries/writes under string aliases and produces a
//! `shamir_query_types::batch::BatchRequest`. Each `query()`/`insert()`/…
//! call returns a typed `Handle` whose `column()`, `row()`, `first()`,
//! `all()` methods emit `FilterValue::QueryRef` values that the planner
//! treats as inter-query dependencies.

#[allow(clippy::module_inception)]
mod batch;
mod build_error;
mod durability;
mod handle;
mod into_batch_op;
mod isolation;

pub use batch::*;
pub use build_error::*;
pub use durability::*;
pub use handle::*;
pub use into_batch_op::*;
pub use isolation::*;

#[cfg(test)]
mod tests;
