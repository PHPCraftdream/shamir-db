//! Typed extraction helpers for `BatchResponse`.
//!
//! The `BatchResponseExt` extension trait adds ergonomic, typed access to
//! query results — by string alias or by a `Handle` obtained from the batch
//! builder. It also exposes transaction-outcome helpers (`is_committed`,
//! `abort_reason`) and the execution plan.
//!
//! Re-exports `TransactionInfo` and `QueryResult` for caller convenience so
//! downstream code does not need a direct `shamir-query-types` import.

mod batch_response_ext;

pub use batch_response_ext::*;

// Re-export for caller convenience.
pub use shamir_query_types::batch::TransactionInfo as TxInfo;
pub use shamir_query_types::read::QueryResult as QResult;

#[cfg(test)]
mod tests;
