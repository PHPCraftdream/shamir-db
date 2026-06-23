//! Executor abstraction traits for the batch query engine.
//!
//! These traits are the thin contracts that decouple `QueryRunner`
//! (and the free `execute_batch*` functions) from the concrete
//! implementations that live in `shamir-db` / `shamir-server`.

use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::query::TableRef;
use crate::table::TableManager;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::CallOp;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use shamir_types::types::common::TMap;

/// Trait for resolving table references to TableManager instances.
#[async_trait::async_trait]
pub trait TableResolver: Send + Sync {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager>;

    /// Resolve a repository by name to its [`crate::repo::RepoInstance`].
    ///
    /// Used by tx-aware paths to obtain the per-repo coordinator
    /// (gate, WAL, commit lifecycle). Cross-repo guard upstream
    /// guarantees `repo_name` is well-defined for transactional batches.
    async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::repo::RepoInstance>;

    /// Return the per-DB scalar resolver (user + builtin layers).
    ///
    /// Default: built-ins only (no user-registered scalars). Overridden
    /// by `DbTableResolver` when user scalars are registered on the
    /// database instance.
    fn scalar_resolver(&self) -> ScalarResolver {
        ScalarResolver::builtins_only()
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
