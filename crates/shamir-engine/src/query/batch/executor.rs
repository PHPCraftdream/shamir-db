//! Batch query executor.
//!
//! Executes a BatchPlan stage by stage, passing results between
//! dependent queries via FilterContext::resolved_refs.

use std::time::Instant;

#[cfg(test)]
use crate::query::auth::SessionPermissions;
use crate::query::batch::{
    BatchError, BatchOp, BatchPlan, BatchRequest, BatchResponse, QueryEntry,
};
#[cfg(test)]
use crate::query::filter::Filter;
use crate::query::filter::FilterContext;
use crate::query::read::{QueryResult, QueryStats};
use crate::query::write::WriteResult;
use crate::query::TableRef;
use crate::table::TableManager;
use shamir_query_types::CallOp;
use shamir_storage::error::DbResult;
use shamir_types::access::{authorize, Action, Actor, ResourcePath};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::InnerValue;

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

/// Execute a batch request against a table resolver.
///
/// 1. Plans the execution (topological sort into parallel stages)
/// 2. Executes each stage, passing results to dependent queries
/// 3. Filters results based on return_all / return_only
///
/// `actor` is threaded from the facade (R2) and carried into every
/// resource-touch point via [`QueryRunner`]. `db_name` provides the
/// database scope for [`ResourcePath`] construction.
pub async fn execute_batch(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: Actor,
    db_name: &str,
) -> Result<BatchResponse, BatchError> {
    execute_batch_impl(
        request,
        resolver,
        admin,
        invoker,
        actor,
        db_name,
        0,
        &new_map(),
    )
    .await
}

/// Internal entry point that carries the sub-batch recursion state.
///
/// - `depth` — current nesting level (0 at the public entry).
/// - `params` — injected `$param` bindings from the outer batch's
///   `BatchOp::Batch.bind` map, resolved to concrete `InnerValue`s.
///
/// Called recursively by [`QueryRunner::run`] when it encounters a
/// `BatchOp::Batch` entry; callers above the public entry always pass
/// `depth = 0` and an empty `params` map.
///
/// Returns a boxed future because the function is mutually recursive
/// (QueryRunner::run → execute_batch_impl → execute_plan_impl →
/// execute_single_impl → QueryRunner::run) and Rust requires boxing
/// to give the async state machine a statically known size.
#[allow(clippy::too_many_arguments)]
fn execute_batch_impl<'a>(
    request: &'a BatchRequest,
    resolver: &'a dyn TableResolver,
    admin: Option<&'a dyn AdminExecutor>,
    invoker: Option<&'a dyn FunctionInvoker>,
    actor: Actor,
    db_name: &'a str,
    depth: usize,
    params: &'a TMap<String, InnerValue>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<BatchResponse, BatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let start = Instant::now();

        // 4.C: cross-repo guard for transactional batches.
        if request.transactional {
            let repos = shamir_query_types::batch::distinct_repos(&request.queries);
            if repos.len() > 1 {
                let mut repos: Vec<String> = repos.into_iter().collect();
                repos.sort();
                return Err(BatchError::CrossRepoNotSupported { repos });
            }
        }

        // 1. Plan
        let plan =
            shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits)?;

        // 2. Validate: all referenced tables exist (skip admin ops)
        validate_tables(&request.queries, resolver).await?;

        // 2b. Validate: filter nesting depth (DoS guard)
        validate_filter_depth(&request.queries)?;

        let mut plan = plan;

        // 3. Execute — branch on transactional.
        let (all_results, tx_info) = if request.transactional {
            execute_transactional_impl(
                request, &mut plan, resolver, admin, invoker, &actor, db_name, depth, params,
            )
            .await?
        } else {
            let r = execute_plan_impl(
                &mut plan,
                &request.queries,
                resolver,
                admin,
                invoker,
                &actor,
                db_name,
                depth,
                params,
            )
            .await?;
            (r, None)
        };

        // 3.5. Durability: if `synced`, flush every distinct repo the batch
        // touched before building the response so the write survives an
        // immediate hard crash.
        if request.durability.as_deref() == Some("synced") {
            let repos = shamir_query_types::batch::distinct_repos(&request.queries);
            for repo_name in repos {
                let repo = resolver.resolve_repo(&repo_name).await.map_err(|e| {
                    BatchError::QueryError {
                        alias: String::new(),
                        message: format!("resolve_repo({}): {}", repo_name, e),
                        code: None,
                    }
                })?;
                repo.synced_flush()
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: String::new(),
                        message: format!("synced flush {}/{}: {}", db_name, repo_name, e),
                        code: None,
                    })?;
            }
        }

        // 4. Filter results for response
        let results = filter_results(all_results, request);

        let elapsed = start.elapsed();

        Ok(BatchResponse {
            id: request.id.clone(),
            results,
            execution_plan: std::mem::take(&mut plan.stages),
            execution_time_us: elapsed.as_micros() as u64,
            transaction: tx_info,
        })
    }) // end Box::pin
}

/// Execute a batch request with permission checks.
///
/// Same as [`execute_batch`] but runs `SessionPermissions::check_batch`
/// before planning/execution. Returns `BatchError::QueryError` if any
/// operation is denied.
///
/// **NOTE (architectural status):** This role-matrix RBAC +
/// `row_filter` RLS path is **test-only scaffolding**. The live
/// server access model is the **Shomer DAC** (owner/group/mode),
/// enforced via `ShamirDb::execute_as` -> `authorize_access` ->
/// `permits`. Groups replace roles; row-level security will be a
/// future Shomer `ResourcePath::Record`-level feature. This
/// function is retained for engine-level unit tests only.
#[cfg(test)]
pub async fn execute_batch_with_permissions(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    permissions: &SessionPermissions,
    db_name: &str,
) -> Result<BatchResponse, BatchError> {
    // 0. Permission check — fail fast before any work
    permissions
        .check_batch(&request.queries, db_name)
        .map_err(|(alias, action, resource)| BatchError::QueryError {
            alias,
            message: format!("Permission denied: {:?} on {:?}", action, resource),
            code: None,
        })?;

    // Stage B-1: enforce row-level security. `row_filter()` was computed but
    // never applied — AND each op's merged row_filter into its WHERE clause so a
    // row_filter grant actually restricts the rows a Read/Update/Delete touches.
    let mut request = request.clone();
    for (_alias, entry) in request.queries.iter_mut() {
        if let Some(rf) = permissions.row_filter_for_op(&entry.op, db_name) {
            apply_row_filter(&mut entry.op, rf);
        }
    }
    execute_batch(&request, resolver, admin, None, Actor::System, db_name).await
}

/// AND a row-level-security filter into a data op's WHERE clause.
/// Read/Update/Delete are restricted; other ops are left unchanged
/// (Insert/Set row-match validation is a separate follow-up).
#[cfg(test)]
fn apply_row_filter(op: &mut BatchOp, rf: Filter) {
    match op {
        BatchOp::Read(q) => q.r#where = Some(and_combine(q.r#where.take(), rf)),
        BatchOp::Update(u) => u.where_clause = Some(and_combine(u.where_clause.take(), rf)),
        BatchOp::Delete(d) => {
            let existing = d.where_clause.clone();
            d.where_clause = Filter::And {
                filters: vec![existing, rf],
            };
        }
        _ => {}
    }
}

/// Combine an optional existing filter with a row filter via AND.
#[cfg(test)]
fn and_combine(existing: Option<Filter>, rf: Filter) -> Filter {
    match existing {
        Some(f) => Filter::And {
            filters: vec![f, rf],
        },
        None => rf,
    }
}

/// Validate that all referenced tables exist before execution.
///
/// Fails fast with a clear error if any table is not found, rather than
/// discovering it mid-execution after some operations have already run.
///
/// Tables/repos that are **created** by this same batch are exempted:
/// the DDL will materialise them before the DML runs (enforced by
/// `after` ordering edges).
async fn validate_tables(
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
) -> Result<(), BatchError> {
    // Phase 1: collect tables/repos being created in this batch so we
    // can skip the existence check for them.
    let (created_tables, created_repos) = tables_created_in_batch(queries);

    // Phase 2: validate remaining table refs.
    let mut seen = shamir_types::types::common::new_set::<String>();
    for (alias, entry) in queries {
        if let Some(table_ref) = entry.op.table_ref() {
            let key = format!("{}/{}", table_ref.repo, table_ref.table);

            // Skip if this table is created by the batch itself.
            if created_tables.contains(&key) || created_repos.contains(&table_ref.repo) {
                continue;
            }

            if seen.insert(key) {
                resolver
                    .resolve(table_ref)
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: alias.clone(),
                        message: format!(
                            "Table '{}' in repo '{}' not found: {}",
                            table_ref.table, table_ref.repo, e
                        ),
                        code: None,
                    })?;
            }
        }
    }
    Ok(())
}

/// Scan batch entries and return:
///  - `created_tables`: set of `"repo/table"` keys for `CreateTable` ops.
///  - `created_repos`: set of repo names for `CreateRepo` ops (any table
///    inside that repo is implicitly "being created").
fn tables_created_in_batch(
    queries: &TMap<String, QueryEntry>,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let mut created_tables = std::collections::HashSet::new();
    let mut created_repos = std::collections::HashSet::new();

    for entry in queries.values() {
        match &entry.op {
            BatchOp::CreateTable(ct) => {
                let key = format!("{}/{}", ct.repo, ct.create_table);
                created_tables.insert(key);
            }
            BatchOp::CreateRepo(cr) => {
                created_repos.insert(cr.create_repo.clone());
            }
            _ => {}
        }
    }

    (created_tables, created_repos)
}

/// Validate that no filter in the batch exceeds the nesting depth cap.
fn validate_filter_depth(queries: &TMap<String, QueryEntry>) -> Result<(), BatchError> {
    for (alias, entry) in queries {
        let filters: Vec<&shamir_query_types::filter::Filter> = match &entry.op {
            BatchOp::Read(q) => q.r#where.iter().collect(),
            BatchOp::Delete(d) => vec![&d.where_clause],
            BatchOp::Update(u) => u.where_clause.iter().collect(),
            _ => vec![],
        };
        for f in filters {
            if let Err(e) = shamir_query_types::filter::check_filter_depth(f) {
                return Err(BatchError::QueryError {
                    alias: alias.clone(),
                    message: e,
                    code: None,
                });
            }
        }
    }
    Ok(())
}

/// Execute a planned batch stage by stage.
///
/// For each stage, executes all queries sequentially within a stage.
/// Each query's FilterContext gets only the resolved_refs from its
/// declared dependencies — not all accumulated results.
///
/// **Note on parallelism.** The planner labels independent queries
/// within one stage with the intent that they run in parallel.
/// Driving them concurrently on a single task via
/// `futures::future::try_join_all` was tried and measured as a
/// no-op on in-memory CPU-bound workloads — there are no await
/// suspension points inside the queries that would yield to peers.
/// Real parallelism needs `tokio::spawn`-per-query, which in turn
/// needs `Arc<dyn TableResolver>` / `Arc<dyn AdminExecutor>` (or a
/// scoped-spawn helper); kept out of scope for now and tracked as a
/// future opt.
#[allow(clippy::too_many_arguments)]
async fn execute_plan_impl(
    plan: &mut BatchPlan,
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    depth: usize,
    params: &TMap<String, InnerValue>,
) -> Result<TMap<String, QueryResult>, BatchError> {
    let mut all_results: TMap<String, QueryResult> = new_map();

    for stage in &plan.stages {
        for alias in stage {
            let entry = queries.get(alias).ok_or_else(|| BatchError::QueryError {
                alias: alias.clone(),
                message: "Query entry not found".to_string(),
                code: None,
            })?;

            // Build resolved_refs with ONLY declared dependencies
            let deps = plan.dependencies.get(alias);
            let resolved_refs = build_resolved_refs(&all_results, deps);

            let result = execute_single_impl(
                alias,
                entry,
                resolver,
                admin,
                invoker,
                &resolved_refs,
                actor,
                db_name,
                depth,
                params,
            )
            .await?;
            all_results.insert(alias.clone(), result);
        }
    }

    Ok(all_results)
}

/// tx-aware variant of [`execute_plan_impl`].
///
/// Uses `QueryRunner` with `Some(&mut tx)` so each mutation routes
/// through `execute_*_tx`. Reads route through `TableManager::read_tx`
/// with a shared `&TxContext` (Vector I.1), so a Serializable batch's
/// SELECT populates the read-set and SSI write-skew detection is live
/// end-to-end through this wire path.
#[allow(clippy::too_many_arguments)]
async fn execute_plan_tx(
    plan: &mut BatchPlan,
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    tx: &mut shamir_tx::TxContext,
) -> Result<TMap<String, QueryResult>, BatchError> {
    execute_plan_tx_impl(
        plan,
        queries,
        resolver,
        admin,
        invoker,
        actor,
        db_name,
        tx,
        0,
        &new_map(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_plan_tx_impl(
    plan: &mut BatchPlan,
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    tx: &mut shamir_tx::TxContext,
    depth: usize,
    params: &TMap<String, InnerValue>,
) -> Result<TMap<String, QueryResult>, BatchError> {
    let mut all_results: TMap<String, QueryResult> = new_map();

    for stage in &plan.stages {
        for alias in stage {
            let entry = queries.get(alias).ok_or_else(|| BatchError::QueryError {
                alias: alias.clone(),
                message: "Query entry not found".to_string(),
                code: None,
            })?;

            let deps = plan.dependencies.get(alias);
            let resolved_refs = build_resolved_refs(&all_results, deps);

            let mut runner = QueryRunner {
                resolver,
                admin,
                invoker,
                tx: Some(&mut *tx),
                actor: actor.clone(),
                db_name,
                depth,
                params,
            };
            let result = runner.run(alias, entry, &resolved_refs).await?;
            all_results.insert(alias.clone(), result);
        }
    }

    Ok(all_results)
}

/// Open a tx, execute the plan inside it, commit (or propagate abort).
///
/// Returns the per-query results AND the populated TransactionInfo
/// (committed or aborted with reason).
#[allow(clippy::too_many_arguments)]
async fn execute_transactional_impl(
    request: &BatchRequest,
    plan: &mut BatchPlan,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    depth: usize,
    params: &TMap<String, InnerValue>,
) -> Result<
    (
        TMap<String, QueryResult>,
        Option<shamir_query_types::batch::TransactionInfo>,
    ),
    BatchError,
> {
    // Determine target repo (cross-repo guard already enforced single).
    let repos = shamir_query_types::batch::distinct_repos(&request.queries);
    let repo_name = repos.into_iter().next().unwrap_or_default();

    if repo_name.is_empty() {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: "transactional batch has no data ops to target a repo".into(),
            code: None,
        });
    }

    let repo = resolver
        .resolve_repo(&repo_name)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("resolve_repo({}): {}", repo_name, e),
            code: None,
        })?;

    // Parse isolation.
    let iso = match request.isolation.as_deref() {
        Some("serializable") => shamir_tx::IsolationLevel::Serializable,
        Some("pessimistic") | Some("level3") => shamir_tx::IsolationLevel::Pessimistic,
        _ => shamir_tx::IsolationLevel::Snapshot,
    };

    let (mut tx, _guard) = repo
        .begin_tx(iso)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: format!("begin_tx: {}", e),
            code: None,
        })?;
    // Thread the actor into the tx for commit-time provenance (R2).
    tx.set_actor(actor.clone());
    let _snapshot_version = tx.snapshot_version;
    let tx_id = tx.tx_id.0;

    // Execute plan in tx mode.
    let plan_result = execute_plan_tx_impl(
        plan,
        &request.queries,
        resolver,
        admin,
        invoker,
        actor,
        db_name,
        &mut tx,
        depth,
        params,
    )
    .await;

    match plan_result {
        Err(plan_err) => {
            // Drop tx without commit = RAII rollback. Release any Level-3
            // pessimistic locks the tx held so blocked txs can proceed (the
            // locks live in the per-table MvccStore, not in TxContext, so
            // dropping TxContext alone does not free them). No-op for
            // Snapshot / Serializable (locked_keys is empty).
            crate::tx::release_pessimistic_locks(&tx, &repo).await;
            let info = shamir_query_types::batch::TransactionInfo::aborted(
                tx_id,
                format!("{:?}", plan_err),
            );
            Ok((new_map(), Some(info)))
        }
        Ok(results) => {
            // Commit.
            match repo.commit_tx(tx).await {
                Ok(outcome) => {
                    // Thread the commit's materialization state to the
                    // client. `Complete` → materialized=true (projections
                    // applied inline); `Deferred` → materialized=false (the
                    // commit is durable via its WAL entry, but projection
                    // was deferred to crash-recovery). A `Deferred` outcome
                    // is still COMMITTED — see `MaterializationState`.
                    let info = shamir_query_types::batch::TransactionInfo::committed(
                        outcome.tx_id,
                        outcome.snapshot_version,
                        outcome.commit_version,
                        outcome.materialized(),
                    );
                    Ok((results, Some(info)))
                }
                Err(commit_err) => {
                    let reason = match commit_err {
                        crate::tx::CommitError::SsiConflict { .. } => "tx_conflict".to_string(),
                        crate::tx::CommitError::PhantomConflict { .. } => "tx_conflict".to_string(),
                        crate::tx::CommitError::Wounded { .. } => "tx_conflict".to_string(),
                        crate::tx::CommitError::UniqueViolation { .. } => {
                            "unique_violation".to_string()
                        }
                        crate::tx::CommitError::Storage(e) => format!("storage: {}", e),
                        crate::tx::CommitError::Expired { elapsed, max } => {
                            format!("tx expired: elapsed {:?} > max {:?}", elapsed, max)
                        }
                    };
                    let info = shamir_query_types::batch::TransactionInfo::aborted(tx_id, reason);
                    Ok((new_map(), Some(info)))
                }
            }
        }
    }
}

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
// `docs/roadmap/PHASE_B_INTERACTIVE_TX.md` §5. These are thin wrappers: no
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
) -> DbResult<(shamir_tx::TxContext, shamir_tx::SnapshotGuard)> {
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
        execution_time_us: elapsed.as_micros() as u64,
        // The tx is still open — there is no commit outcome yet.
        transaction: None,
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

/// Build resolved_refs map containing only the declared dependencies.
fn build_resolved_refs(
    all_results: &TMap<String, QueryResult>,
    deps: Option<&shamir_types::types::common::TSet<String>>,
) -> TMap<String, QueryResult> {
    let mut refs = new_map();
    if let Some(dep_set) = deps {
        for dep_alias in dep_set {
            if let Some(result) = all_results.get(dep_alias) {
                refs.insert(dep_alias.clone(), result.clone());
            }
        }
    }
    refs
}

/// Encapsulates per-query execution context — resolver, admin, optional
/// transaction state, and the [`Actor`] + db name (R2) for the
/// transparent authorization gate.
///
/// In non-tx mode (`tx == None`) runs exactly like the original free
/// function `execute_single`. In tx mode (`tx == Some(&mut TxContext)`)
/// each mutation routes through tx-aware methods (`execute_*_tx`).
///
/// `depth` and `params` support nested `BatchOp::Batch` execution (P3):
/// - `depth` is the current nesting level (0 at the outermost batch).
/// - `params` carries the injected `$param` bindings resolved from the
///   outer batch's `BatchOp::Batch.bind` map for this execution scope.
///
/// Per design decision D9 in
/// `docs/pre-transactional/05-executor-isolation.md`.
pub struct QueryRunner<'a> {
    pub resolver: &'a dyn TableResolver,
    pub admin: Option<&'a dyn AdminExecutor>,
    pub invoker: Option<&'a dyn FunctionInvoker>,
    pub tx: Option<&'a mut shamir_tx::TxContext>,
    pub actor: Actor,
    pub db_name: &'a str,
    /// Current nesting depth (0 at the public entry).
    pub depth: usize,
    /// Injected `$param` bindings for this execution scope.
    pub params: &'a TMap<String, InnerValue>,
}

impl<'a> QueryRunner<'a> {
    /// Build a [`ResourcePath::Table`] for the given table reference.
    fn table_resource(&self, table_ref: &TableRef) -> ResourcePath {
        ResourcePath::Table {
            db: self.db_name.to_string(),
            store: table_ref.repo.clone(),
            table: table_ref.table.clone(),
        }
    }

    /// Execute a single query entry.
    ///
    /// Dispatches by `BatchOp` variant. When `self.tx.is_some()`,
    /// mutation ops (Insert/Update/Delete/Set) route through
    /// `TableManager::execute_*_tx`; read and admin ops are
    /// unchanged.
    ///
    /// Each data op calls [`authorize`] with the appropriate [`Action`]
    /// before performing work (R2 transparent gate — always `Ok`).
    pub async fn run(
        &mut self,
        alias: &str,
        entry: &QueryEntry,
        resolved_refs: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        // Sub-batch — handle before is_admin() so we can recurse rather
        // than delegating to AdminExecutor (which has no recursion seam).
        if let BatchOp::Batch(sub) = &entry.op {
            // Guard: transactional sub-batch inside an already-open tx
            // is not supported (two-phase commit across a shared TxContext
            // is not safe; per design the outer should be non-transactional
            // when it contains transactional sub-batches).
            if sub.batch.transactional && self.tx.is_some() {
                return Err(BatchError::query_coded(
                    alias,
                    "nested_tx_not_supported",
                    "a transactional sub-batch cannot run inside an outer transaction",
                ));
            }

            // Resolve the `bind` map against the CURRENT scope's resolved_refs
            // and params. Each value is a FilterValue — resolve it to an
            // InnerValue using the same machinery as filter evaluation.
            // We use a dummy record (Null) because bind values may only
            // reference $query aliases or literals, not record fields.
            let dummy_record = InnerValue::Null;
            // We need an Interner for FilterContext, but bind values must only
            // be literals or $query refs (not FieldRefs). Use a scratch interner.
            let scratch = shamir_types::core::interner::Interner::new();
            let bind_ctx = FilterContext::new(&scratch, resolved_refs)
                .with_actor(self.actor.clone())
                .with_params(self.params);
            let mut resolved_params: TMap<String, InnerValue> = new_map();
            for (key, fv) in &sub.bind {
                match fv {
                    crate::query::filter::FilterValue::Param { name } => {
                        // $param in a bind value means look up from the
                        // current (outer) scope's params — propagation.
                        let v = self.params.get(name.as_str()).ok_or_else(|| {
                            BatchError::query_coded(
                                alias,
                                "unbound_param",
                                format!("$param '{}' is not bound in the current scope", name),
                            )
                        })?;
                        resolved_params.insert(key.clone(), v.clone());
                    }
                    other => {
                        let v = crate::query::filter::eval::resolve_filter_value(
                            other,
                            &dummy_record,
                            &bind_ctx,
                        )
                        .ok_or_else(|| BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "bind key '{}': cannot resolve filter value {:?}",
                                key, fv
                            ),
                            code: None,
                        })?;
                        resolved_params.insert(key.clone(), v);
                    }
                }
            }

            // Recurse into the sub-batch.
            let inner_response = execute_batch_impl(
                &sub.batch,
                self.resolver,
                self.admin,
                self.invoker,
                self.actor.clone(),
                self.db_name,
                self.depth + 1,
                &resolved_params,
            )
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: format!("sub-batch '{}' failed: {}", alias, e),
                code: e.code().map(str::to_owned),
            })?;

            // Wrap the inner BatchResponse into a QueryResult for the outer
            // $query path resolution.
            //
            // `resolve_query_ref_value` in eval.rs checks `qr.value` first
            // (Call-result path). We store the inner results map as a JSON
            // object in `value` so outer ops can access sub-aliases:
            //   $query @sub[0].records[0].id  — NOT supported (records empty)
            //   $query @sub.alias_name[0].id  — walks value.alias_name[0].id
            //
            // The inner results are already JSON (QueryResult::records are
            // Vec<serde_json::Value>), so we serialise the entire results map
            // directly.
            let value = serde_json::to_value(&inner_response.results).ok();
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value,
            });
        }

        // Admin ops — delegate to AdminExecutor (no tx).
        if entry.op.is_admin() {
            return match self.admin {
                Some(executor) => executor.execute_admin(&entry.op).await,
                None => Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message: "Admin operations not supported in this context".to_string(),
                    code: None,
                }),
            };
        }

        // Call ops — delegate to FunctionInvoker (autocommit, no tx).
        if let BatchOp::Call(call_op) = &entry.op {
            return match self.invoker {
                Some(inv) => inv.invoke_call(call_op, &self.actor, resolved_refs).await,
                None => Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message: "Function calls not supported in this context".to_string(),
                    code: None,
                }),
            };
        }

        let table_ref = entry.op.table_ref().unwrap();
        let resource = self.table_resource(table_ref);

        let table = self
            .resolver
            .resolve(table_ref)
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: e.to_string(),
                code: None,
            })?;

        let interner = table
            .interner()
            .get()
            .await
            .map_err(|e| BatchError::QueryError {
                alias: alias.to_string(),
                message: e.to_string(),
                code: None,
            })?;

        let ctx = FilterContext::new(interner, resolved_refs)
            .with_actor(self.actor.clone())
            .with_params(self.params);

        match &entry.op {
            BatchOp::Read(query) => {
                authorize(&self.actor, &resource, Action::Read).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                // Vector I.1: in a transactional batch route the read through
                // `read_tx` with a SHARED `&TxContext` so the SELECT records
                // into the read-set (Serializable → SSI write-skew detection
                // goes live end-to-end). `as_deref()` reborrows the runner's
                // `&mut TxContext` as `&TxContext`; the read branch never also
                // holds the `&mut`, and queries within a stage run sequentially
                // (no read/write aliasing over the same tx). Non-tx batches
                // keep the original zero-overhead `read` path.
                match self.tx.as_deref() {
                    Some(tx) => table.read_tx(query, &ctx, Some(tx)).await,
                    None => table.read(query, &ctx).await,
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })
            }

            BatchOp::Insert(op) => {
                authorize(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_insert_tx(op, tx).await,
                    None => table.execute_insert(op).await,
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })?;
                Ok(write_result_to_query_result(wr))
            }

            BatchOp::Update(op) => {
                authorize(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_update_tx(op, &ctx, tx).await,
                    None => table.execute_update(op, &ctx).await,
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })?;
                Ok(write_result_to_query_result(wr))
            }

            BatchOp::Delete(op) => {
                authorize(&self.actor, &resource, Action::Delete).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_delete_tx(op, &ctx, tx).await,
                    None => table.execute_delete(op, &ctx).await,
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })?;
                Ok(write_result_to_query_result(wr))
            }

            BatchOp::Set(op) => {
                authorize(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_set_tx(op, tx).await,
                    None => table.execute_set(op).await,
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })?;
                Ok(write_result_to_query_result(wr))
            }

            // Admin ops are handled before this match via is_admin() check
            _ => unreachable!("Admin ops should have been handled earlier"),
        }
    }
}

/// Execute a single query/operation entry.
///
/// Thin wrapper around [`QueryRunner`] with `tx: None`.
#[allow(clippy::too_many_arguments)]
async fn execute_single_impl(
    alias: &str,
    entry: &QueryEntry,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    resolved_refs: &TMap<String, QueryResult>,
    actor: &Actor,
    db_name: &str,
    depth: usize,
    params: &TMap<String, InnerValue>,
) -> Result<QueryResult, BatchError> {
    let mut runner = QueryRunner {
        resolver,
        admin,
        invoker,
        tx: None,
        actor: actor.clone(),
        db_name,
        depth,
        params,
    };
    runner.run(alias, entry, resolved_refs).await
}

/// Convert WriteResult to QueryResult for BatchResponse compatibility.
fn write_result_to_query_result(wr: WriteResult) -> QueryResult {
    QueryResult {
        records: wr.records,
        stats: Some(QueryStats {
            index_used: None,
            records_scanned: wr.affected,
            records_returned: wr.affected,
            execution_time_us: wr.execution_time_us,
        }),
        pagination: None,
        value: None,
    }
}

/// Filter results based on return_all / return_only / return_result flags.
fn filter_results(
    mut all_results: TMap<String, QueryResult>,
    request: &BatchRequest,
) -> TMap<String, QueryResult> {
    if let Some(ref only) = request.return_only {
        let keep: std::collections::HashSet<String> = only.iter().cloned().collect();
        all_results.retain(|alias, _| keep.contains(alias));
        return all_results;
    }

    if !request.return_all {
        all_results.retain(|alias, _| request.queries.get(alias).is_some_and(|e| e.return_result));
    }

    all_results
}
