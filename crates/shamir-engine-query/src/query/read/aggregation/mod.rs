//! Aggregation implementation, split out of the former `read/aggregate.rs`
//! (bodies unchanged). `read/mod.rs` aliases this module under the historical
//! `read::aggregate` path.

pub mod accumulator;
pub mod function_inputs;
pub mod group_execution;
pub mod group_projection;
pub mod validation;

// Public surface of the former `read::aggregate` module (kept stable).
pub use group_execution::{apply_aggregate_all, apply_group_by};
pub use group_execution::{apply_aggregate_all_with_context, try_apply_group_by};
pub use validation::validate_aggregate_select;
// Items whose historical visibility was `read`-tree only (former `pub(super)`
// in `read::aggregate`); used by the read tests.
#[cfg(test)]
pub(in crate::query::read) use accumulator::AggAccum;
