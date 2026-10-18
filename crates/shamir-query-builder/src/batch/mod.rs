//! Batch assembler + typed `Handle`/`RowRef` dependency references.
//!
//! `Batch` accumulates queries/writes under string aliases and produces a
//! `shamir_query_types::batch::BatchRequest`. Each `query()`/`insert()`/…
//! call returns a typed `Handle` whose `column()`, `row()`, `first()`,
//! `all()` methods emit `FilterValue::QueryRef` values that the planner
//! treats as inter-query dependencies.

#[allow(clippy::module_inception)]
mod batch;
mod op;
pub use op::{build_error, into_batch_op, try_into_batch_op};
mod runtime;
pub use runtime::{durability, handle, isolation, subscribe};
pub use vis::{index_visibility, visibility};
mod vis;

pub use batch::*;
pub use build_error::*;
pub use durability::*;
pub use handle::*;
pub use index_visibility::*;
pub use into_batch_op::*;
pub use isolation::*;
pub use subscribe::{SourceBuilder, Subscribe};
pub use try_into_batch_op::*;

#[cfg(test)]
mod tests;
