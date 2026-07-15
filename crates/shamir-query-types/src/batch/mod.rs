//! Batch-request DTOs + pure planning / reference-parsing logic.
//!
//! - `batch_op` — dispatch enum over all supported operations.
//! - `sub_batch_op` — nested sub-batch with parameter bindings.
//! - `for_each_op` — data-dependent loop: run a nested batch K times, once
//!   per element of `over`, binding the current element to `bind_row`.
//! - `query_entry` — single operation slot + `distinct_repos` helper.
//! - `batch_request` — top-level batch request DTO.
//! - `batch_response` — top-level batch response DTO.
//! - `transaction_info` — MVCC transaction metadata.
//! - `batch_limits` — per-request security / resource limits.
//! - `batch_plan` — topological execution plan with parallel stages.
//! - `batch_error` — errors during batch processing.
//! - `planner` — topological sort of inter-query `$query` references
//!   into parallel execution stages. Pure function over DTOs; no
//!   storage or runtime types involved.
//! - `reference` — parser for `@alias[0].field` reference strings.
//!   Pure string analysis.
//! - `alias` — `extract_base_alias`, the shared alias-reference
//!   normalization used by the planner and (Epic01/B) the query builders.
//! - `edge_kind` — [`EdgeKind`] provenance tag (`Explicit` / `DataFlow` /
//!   `Both`) recorded per dependency edge in `BatchPlan::edge_provenance`.
//!
//! The actual executor (which drives a TableManager and invokes
//! storage backends) stays in `shamir-engine::query::batch::executor`.

pub mod alias;
pub mod batch_error;
pub mod batch_limits;
pub mod batch_op;
pub mod batch_plan;
pub mod batch_request;
pub mod batch_response;
pub mod edge_kind;
pub mod for_each_op;
pub mod interner_delta;
pub mod planner;
pub mod query_entry;
pub mod reference;
pub mod result_encoding;
pub mod sub_batch_op;
pub mod transaction_info;

#[cfg(test)]
mod tests;

pub use alias::extract_base_alias;
pub use batch_error::BatchError;
pub use batch_limits::BatchLimits;
pub use batch_op::BatchOp;
pub use batch_plan::BatchPlan;
pub use batch_request::BatchRequest;
pub use batch_response::BatchResponse;
pub use edge_kind::EdgeKind;
pub use for_each_op::ForEachOp;
pub use interner_delta::InternerDelta;
pub use planner::BatchPlanner;
pub use query_entry::{distinct_repos, QueryEntry};
pub use reference::{QueryPath, QueryReference, ReferenceParseError};
pub use result_encoding::ResultEncoding;
pub use sub_batch_op::SubBatchOp;
pub use transaction_info::TransactionInfo;
