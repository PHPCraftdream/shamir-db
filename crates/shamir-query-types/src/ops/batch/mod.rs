//! Batch-request DTOs + pure planning / reference-parsing logic.
//!
//! - `batch_op` — dispatch enum over all supported operations.
//! - `sub_batch_op` — nested sub-batch with parameter bindings.
//! - `for_each_op` — data-dependent loop: run a nested batch K times, once
//!   per element of `over`, binding the current element to `bind_row`.
//! - `query_entry` — single operation slot + `distinct_repos` /
//!   `collect_required_access` helpers.
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

mod core;
mod ops;
mod plan;
mod req;
mod vis;

pub use core::alias;
pub use core::batch_error;
pub use core::interner_delta;
pub use core::reference;
pub use ops::batch_op;
pub use ops::for_each_op;
pub use ops::query_entry;
pub use ops::sub_batch_op;
pub use plan::batch_plan;
pub use plan::edge_kind;
pub use plan::exec_policy;
pub use plan::limit_kind;
pub use plan::planner;
pub use plan::tree_visitor;
pub use req::batch_limits;
pub use req::batch_request;
pub use req::batch_response;
pub use req::options;
pub use req::result_encoding;
pub use req::transaction_info;
pub use vis::visibility_pending_reason;
pub use vis::visibility_state;
pub use vis::visibility_wait;

#[cfg(test)]
mod tests;

pub use alias::extract_base_alias;
pub use batch_error::{BatchError, ResyncRequiredDetail};
pub use batch_limits::BatchLimits;
pub use batch_op::BatchOp;
pub use batch_plan::BatchPlan;
pub use batch_request::BatchRequest;
pub use batch_response::BatchResponse;
pub use edge_kind::EdgeKind;
pub use exec_policy::ExecPolicy;
pub use for_each_op::ForEachOp;
pub use interner_delta::InternerDelta;
pub use limit_kind::LimitKind;
pub use options::{IndexVisibility, IsolationLevel, RequestDurability};
pub use planner::BatchPlanner;
pub use query_entry::{collect_required_access, distinct_repos, QueryEntry};
pub use reference::{QueryPath, QueryReference, ReferenceParseError};
pub use result_encoding::ResultEncoding;
pub use sub_batch_op::SubBatchOp;
pub use transaction_info::TransactionInfo;
pub use tree_visitor::{BatchTreeEntry, BatchTreeVisitor};
pub use visibility_pending_reason::VisibilityPendingReason;
pub use visibility_state::VisibilityState;
pub use visibility_wait::VisibilityWait;
