//! Sorted (B-tree-by-value) index manager.
//!
//! Parallel to the hash-based `IndexManager`. Where hash indexes
//! answer **equality** lookups (`field == value`), sorted indexes
//! answer **range / order / min** queries by encoding the indexed
//! value into bytes that sort the same way the value does (see
//! `shamir_types::core::sort_codec`) and storing one info-store
//! record per `(value, record_id)` pair.
//!
//! What's supported in this first cut:
//! - Single-field index over a scalar column (Int / Float / String /
//!   Bool / U64).
//! - Range queries: between / gt / gte / lt / lte.
//! - `order by field asc + limit K` (forward scan, stop after K).
//! - `min(field)` (first record from prefix scan).
//!
//! Not yet:
//! - `max(field)`, `order by desc` — needs reverse iteration on the
//!   Store trait (next).
//! - Composite sorted index over multiple columns.

mod covering;
mod ddl;
mod inner;
mod plan;
mod scan;
#[cfg(test)]
mod tests;

pub use crate::base_index::sorted_index_definition::SortedIndexDefinition;
// Re-export so existing callers that import `SortedIndexDefinition` from this
// module continue to compile unchanged after the type moved to its own file.
pub use covering::decode_covering_projection;
pub use inner::SortedIndexManager;
