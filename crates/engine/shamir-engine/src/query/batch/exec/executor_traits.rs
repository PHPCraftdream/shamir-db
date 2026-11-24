//! Executor abstraction traits for the batch query engine.
//!
//! These traits are the thin contracts that decouple `QueryRunner`
//! (and the free `execute_batch*` functions) from the concrete
//! implementations that live in `shamir-db` / `shamir-server`.
//! The table-side [`TableResolver`] contract lives in
//! `table::support::table_resolver` and is re-exported here.

use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use shamir_engine_query::query::filter::read_budget::QueryMetrics;
use shamir_query_types::CallOp;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use shamir_types::types::common::TMap;

pub use crate::table::TableResolver;

/// Repo-scoped resolution port for tx-aware batch paths.
///
/// Extends [`TableResolver`] with the one method that reaches above the
/// table layer; after the engine's crate split this port stays on the
/// batch (query) side, so its return type may follow the tx-facing
/// coordinator without dragging the table-side contract along.
#[async_trait::async_trait]
pub trait BatchResolver: TableResolver {
    /// Resolve a repository by name to its [`crate::tx::RepoCore`] — the
    /// tx-layer aggregate (gate, WAL, commit lifecycle), a cheap `Clone`.
    ///
    /// Used by tx-aware paths to obtain the per-repo coordinator.
    /// Cross-repo guard upstream guarantees `repo_name` is well-defined
    /// for transactional batches.
    async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::tx::RepoCore>;

    /// Optional catalogue metadata probe used to preflight table reads.
    /// `None` means the resolver cannot answer without resolving the table.
    fn table_read_exists(
        &self,
        _db_name: &str,
        _repo_name: &str,
        _table_name: &str,
    ) -> Option<bool> {
        None
    }

    /// Optional per-repository telemetry sink for query dispatch. Implementors
    /// that do not own engine metrics may keep the default no-op behavior.
    fn query_metrics(&self, _repo_name: &str) -> Option<std::sync::Arc<dyn QueryMetrics>> {
        None
    }
}

/// Trait for executing admin (DDL) operations.
#[async_trait::async_trait]
pub trait AdminExecutor: Send + Sync {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError>;
}

/// Trait for invoking stored procedures / callable getter-functions.
///
/// The engine executor does not know how to run a WASM function — that
/// lives in `shamir-db` (`ShamirDb::invoke_function_in_db_as`). This
/// trait is the thin channel that lets `QueryRunner::run` dispatch a
/// `BatchOp::Call` to the facade without a circular dependency.
///
/// Injected alongside [`AdminExecutor`] (same pattern, same lifecycle).
#[async_trait::async_trait]
pub trait FunctionInvoker: Send + Sync {
    /// Invoke a stored procedure named by `op.call` with the given
    /// positional `op.params` (already resolved — `$query` refs
    /// expanded by the executor) under authority of `actor`, returning
    /// the function's `QueryValue` mapped into a `QueryResult.value`.
    async fn invoke_call(
        &self,
        op: &CallOp,
        actor: &Actor,
        resolved_refs: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError>;
}
