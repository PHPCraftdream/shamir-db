use shamir_query_types::batch::ResultEncoding;

use shamir_types::access::Actor;
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use crate::query::batch::batch_execute::execute_plan_tx_impl;
use crate::query::batch::exec_scope::ExecScope;
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::param_subst::WriteValueError;
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;

/// Run a nested `Batch`/`ForEach` body (which is NOT itself marked
/// `transactional` — the only reachable case once the `nested_tx_not_supported`
/// guard has already rejected `body.transactional && tx.is_some()`) so its
/// writes are genuine participants in the ALREADY-OPEN outer transaction
/// `tx`, instead of each write silently committing independently via
/// `execute_batch_impl`'s non-transactional (implicit-per-op-tx) path (#661).
///
/// `plan` is the body's ALREADY-BUILT [`BatchPlan`](shamir_query_types::batch::BatchPlan)
/// — the caller ran `plan_and_validate_batch` (`BatchPlanner::plan` +
/// `validate_tables` + `validate_filter_depth`) beforehand. For a single
/// `BatchOp::Batch` recursion the caller does this once, immediately before
/// calling here (byte-identical to the old inline behavior). For `ForEach`,
/// the caller does this ONCE before the iteration loop and reuses the same
/// `plan` across every iteration — the body's shape never changes between
/// iterations, only the bound `$param` values do, so re-planning/
/// re-validating per iteration was pure redundant work (see the `ForEach`
/// handling below).
///
/// Drives the plan directly through [`execute_plan_tx_impl`], reusing `tx`
/// — so a failure anywhere in the nested body propagates as an `Err` out of
/// the SAME `TxContext` the outer `execute_transactional_impl` already
/// knows how to roll back on `Err` (RAII: the tx is dropped without commit),
/// with zero changes needed to that commit/abort logic.
///
/// Returns the per-alias results map (the same shape `BatchResponse::results`
/// carries), which callers wrap into the outer `QueryResult.value` exactly
/// like the `execute_batch_impl` path already does.
///
/// Returns a boxed future for the same reason `execute_batch_impl` does:
/// this function is mutually recursive with `QueryRunner::run` (`run` calls
/// this fn for a nested `Batch`/`ForEach` body reusing the outer tx, which
/// calls `execute_plan_tx_impl`, which constructs a `QueryRunner` and calls
/// `run` again for the nested body's own entries) — Rust requires boxing to
/// give the async state machine a statically known size.
type NestedTxBodyFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<TMap<String, QueryResult>, BatchError>> + Send + 'a,
    >,
>;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_nested_body_in_outer_tx<'a>(
    body: &'a crate::query::batch::BatchRequest,
    plan: &'a mut shamir_query_types::batch::BatchPlan,
    resolver: &'a dyn TableResolver,
    admin: Option<&'a dyn AdminExecutor>,
    invoker: Option<&'a dyn FunctionInvoker>,
    actor: &'a Actor,
    db_name: &'a str,
    tx: &'a mut shamir_tx::TxContext,
    depth: usize,
    params: &'a TMap<String, QueryValue>,
    result_encoding: ResultEncoding,
    scope: &'a ExecScope,
) -> NestedTxBodyFuture<'a> {
    Box::pin(async move {
        // #666 checkpoint: nested-body entry — mirrors the check at the top
        // of `execute_batch_impl` for the non-tx recursion path.
        scope.check()?;

        execute_plan_tx_impl(
            plan,
            &body.queries,
            resolver,
            admin,
            invoker,
            actor,
            db_name,
            tx,
            depth,
            params,
            result_encoding,
            scope,
        )
        .await
    })
}

/// Map a [`WriteValueError`] (from `resolve_write_value`) to the
/// [`BatchError`] shape every Insert/Update/Set dispatch arm already returns
/// for its `$param`-only substitution failures — `unbound_param` keeps its
/// pre-existing code, `malformed_marker` is new (a `$query`/`$fn`/`$cond`/
/// `$expr` marker that failed to resolve).
pub(super) fn write_value_error_to_batch_error(alias: &str, err: WriteValueError) -> BatchError {
    match err {
        WriteValueError::UnboundParam(name) => BatchError::query_coded(
            alias,
            "unbound_param",
            format!("$param '{}' is not bound in this sub-batch", name),
        ),
        WriteValueError::MalformedMarker(message) => {
            BatchError::query_coded(alias, "malformed_marker", message)
        }
    }
}
