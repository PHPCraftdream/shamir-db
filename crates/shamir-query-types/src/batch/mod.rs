//! Batch-request DTOs + pure planning / reference-parsing logic.
//!
//! - `types` — wire-shareable DTOs (BatchRequest / BatchResponse /
//!   BatchOp / BatchLimits / BatchError / BatchPlan / QueryEntry /
//!   TransactionInfo).
//! - `planner` — topological sort of inter-query `$query` references
//!   into parallel execution stages. Pure function over DTOs; no
//!   storage or runtime types involved.
//! - `reference` — parser for `@alias[0].field` reference strings.
//!   Pure string analysis.
//!
//! The actual executor (which drives a TableManager and invokes
//! storage backends) stays in `shamir-engine::query::batch::executor`.

pub mod planner;
pub mod reference;
pub mod types;

#[cfg(test)]
mod tests;

pub use planner::BatchPlanner;
pub use reference::{QueryPath, QueryReference, ReferenceParseError};
pub use types::{
    distinct_repos, BatchError, BatchLimits, BatchOp, BatchPlan, BatchRequest, BatchResponse,
    QueryEntry, TransactionInfo,
};
