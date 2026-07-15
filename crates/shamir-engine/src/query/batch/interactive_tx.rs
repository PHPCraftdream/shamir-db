use std::time::Instant;

use crate::query::batch::batch_execute::{execute_plan_tx, filter_results};
use crate::query::batch::batch_validate::validate_tables;
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::{BatchError, BatchRequest, BatchResponse};
use shamir_types::access::Actor;

// ===========================================================================
// Phase B — interactive (multi-call) transactions
//
// Phase A's `execute_transactional` opens a tx, runs ONE batch's plan, and
// commits — all inside a single call. Phase B factors that closed cycle into
// three reusable pieces so a `TxContext` can be parked server-side (the
// interactive-tx registry) and driven across multiple client round-trips:
//
//   * `open_interactive_tx`   — BEGIN: mint the tx + snapshot guard.
//   * `execute_in_open_tx`    — EXECUTE: run one batch's plan against the
//                               EXISTING tx, NO commit (the tx stays open).
//   * `commit_interactive_tx` — COMMIT: run the Phase-A commit pipeline.
//
// ROLLBACK needs no function — dropping the parked `TxContext` is the RAII
// rollback (no storage side effects). See
// `docs/dev-artifacts/roadmap/PHASE_B_INTERACTIVE_TX.md` §5. These are thin wrappers: no
// new execution logic. `execute_in_open_tx` mirrors `execute_batch`
// (plan → validate → run → filter) but routes through `execute_plan_tx`
// with the caller's `&mut TxContext` and returns `transaction: None` because
// the tx is still open — the commit outcome is produced later by
// `commit_interactive_tx`.
// ===========================================================================

/// BEGIN an interactive transaction: open the MVCC snapshot and mint the
/// `TxContext`.
///
/// The returned `SnapshotGuard` MUST be parked alongside the `TxContext` and
/// held until commit/rollback — dropping it early lets GC reclaim versions
/// the open tx still needs (see [`crate::repo::RepoInstance::begin_tx`]).
pub async fn open_interactive_tx(
    repo: &crate::repo::RepoInstance,
    iso: shamir_tx::IsolationLevel,
) -> shamir_storage::error::DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
    repo.begin_tx(iso).await
}

/// EXECUTE one batch inside an already-open interactive tx, WITHOUT
/// committing.
///
/// Mirrors [`execute_batch`] but threads the caller's existing `tx` through
/// [`execute_plan_tx`]; the returned `BatchResponse` always has
/// `transaction: None` (the tx remains open — the commit outcome is reported
/// later by [`commit_interactive_tx`]).
///
/// The single-repo guard is enforced here exactly as in [`execute_batch`];
/// the caller additionally asserts that the batch targets the SAME repo the
/// handle is pinned to (the engine tx is committed against one repo).
pub async fn execute_in_open_tx(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    tx: &mut shamir_tx::TxContext,
) -> Result<BatchResponse, BatchError> {
    let start = Instant::now();

    // Single-repo guard (mirrors `execute_batch`). The handle pins exactly
    // one repo; a batch spanning more than one repo is rejected.
    let repos = shamir_query_types::batch::distinct_repos(&request.queries);
    if repos.len() > 1 {
        let mut repos: Vec<String> = repos.into_iter().collect();
        repos.sort();
        return Err(BatchError::CrossRepoNotSupported { repos });
    }

    let mut plan =
        shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits)?;
    validate_tables(&request.queries, resolver).await?;

    let all_results = execute_plan_tx(
        &mut plan,
        &request.queries,
        resolver,
        admin,
        invoker,
        actor,
        db_name,
        tx,
    )
    .await?;

    let results = filter_results(all_results, request);
    let elapsed = start.elapsed();

    Ok(BatchResponse {
        id: request.id.clone(),
        results,
        execution_plan: std::mem::take(&mut plan.stages),
        edge_provenance: std::mem::take(&mut plan.edge_provenance),
        execution_time_us: elapsed.as_micros() as u64,
        // The tx is still open — there is no commit outcome yet.
        transaction: None,
        // Ambient interner deltas are attached server-side (shamir-db); empty here.
        interner_delta: Default::default(),
    })
}

/// COMMIT an interactive tx: run the full Phase-A commit pipeline.
///
/// The caller has already removed the `TxContext` from the registry (and
/// dropped any per-handle lock); the `SnapshotGuard` is dropped by the
/// caller only AFTER this returns — the snapshot must stay alive through
/// commit so SSI read-set validation and history reads remain correct.
pub async fn commit_interactive_tx(
    repo: &crate::repo::RepoInstance,
    tx: shamir_tx::TxContext,
) -> Result<crate::tx::TxOutcome, crate::tx::CommitError> {
    repo.commit_tx(tx).await
}
