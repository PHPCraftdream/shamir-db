use std::time::Instant;

use crate::query::batch::batch_validate::{validate_filter_depth, validate_tables};
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::query_runner::{build_resolved_refs, execute_single_impl, QueryRunner};
use crate::query::batch::{BatchError, BatchPlan, BatchRequest, BatchResponse, QueryEntry};
use crate::query::read::QueryResult;
use shamir_collections::TFxSet;
use shamir_tx::CommitVisibility;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, new_map_wc, TMap};
use shamir_types::types::value::InnerValue;

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
pub(super) fn execute_batch_impl<'a>(
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
                // Belt-and-suspenders: also fsync the file WAL spine so the
                // committed entries reach level 3 (the WAL is the source of
                // truth; data-store flush above is derived). No-op for
                // in-memory repos.
                repo.sync_wal().await.map_err(|e| BatchError::QueryError {
                    alias: String::new(),
                    message: format!("synced wal {}/{}: {}", db_name, repo_name, e),
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
            // Ambient interner deltas are attached server-side (shamir-db's
            // execute_as post-processing); the engine layer leaves this empty.
            interner_delta: Default::default(),
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
    permissions: &crate::query::auth::SessionPermissions,
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
fn apply_row_filter(op: &mut crate::query::batch::BatchOp, rf: crate::query::filter::Filter) {
    use crate::query::batch::BatchOp;
    match op {
        BatchOp::Read(q) => q.r#where = Some(and_combine(q.r#where.take(), rf)),
        BatchOp::Update(u) => u.where_clause = Some(and_combine(u.where_clause.take(), rf)),
        BatchOp::Delete(d) => {
            let existing = d.where_clause.clone();
            d.where_clause = crate::query::filter::Filter::And {
                filters: vec![existing, rf],
            };
        }
        _ => {}
    }
}

/// Combine an optional existing filter with a row filter via AND.
#[cfg(test)]
fn and_combine(
    existing: Option<crate::query::filter::Filter>,
    rf: crate::query::filter::Filter,
) -> crate::query::filter::Filter {
    match existing {
        Some(f) => crate::query::filter::Filter::And {
            filters: vec![f, rf],
        },
        None => rf,
    }
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
pub(super) async fn execute_plan_impl(
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
    let mut all_results: TMap<String, QueryResult> = new_map_wc(queries.len());

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
pub(super) async fn execute_plan_tx(
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
pub(super) async fn execute_plan_tx_impl(
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
    let mut all_results: TMap<String, QueryResult> = new_map_wc(queries.len());

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
pub(super) async fn execute_transactional_impl(
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
    // Opt into async-index commit visibility when the wire client requests it.
    // "buffered" (default) and "synced" keep CommitVisibility::Synchronous so
    // existing behaviour is byte-identical.
    if request.durability.as_deref() == Some("async_index") {
        tx.set_visibility(CommitVisibility::AsyncIndex);
    }
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

/// Filter results based on return_all / return_only / return_result flags.
pub(super) fn filter_results(
    mut all_results: TMap<String, QueryResult>,
    request: &BatchRequest,
) -> TMap<String, QueryResult> {
    if let Some(ref only) = request.return_only {
        let keep: TFxSet<String> = only.iter().cloned().collect();
        all_results.retain(|alias, _| keep.contains(alias));
        return all_results;
    }

    if !request.return_all {
        all_results.retain(|alias, _| request.queries.get(alias).is_some_and(|e| e.return_result));
    }

    all_results
}
