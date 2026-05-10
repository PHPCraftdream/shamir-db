//! Batch query executor.
//!
//! Executes a BatchPlan stage by stage, passing results between
//! dependent queries via FilterContext::resolved_refs.

use std::time::Instant;

use crate::table::TableManager;
use crate::query::auth::SessionPermissions;
use crate::query::batch::{
    BatchError, BatchOp, BatchPlan, BatchRequest, BatchResponse, QueryEntry,
};
use crate::query::TableRef;
use crate::query::filter::FilterContext;
use crate::query::read::{QueryResult, QueryStats};
use crate::query::write::WriteResult;
use shamir_storage::error::DbResult;
use shamir_types::types::common::{new_map, TMap};

/// Trait for resolving table references to TableManager instances.
#[async_trait::async_trait]
pub trait TableResolver: Send + Sync {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager>;
}

/// Trait for executing admin (DDL) operations.
#[async_trait::async_trait]
pub trait AdminExecutor: Send + Sync {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError>;
}

/// Execute a batch request against a table resolver.
///
/// 1. Plans the execution (topological sort into parallel stages)
/// 2. Executes each stage, passing results to dependent queries
/// 3. Filters results based on return_all / return_only
pub async fn execute_batch(
    request: &BatchRequest,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
) -> Result<BatchResponse, BatchError> {
    let start = Instant::now();

    // 1. Plan
    let plan = shamir_query_types::batch::BatchPlanner::plan(&request.queries, &request.limits)?;

    // 2. Validate: all referenced tables exist (skip admin ops)
    validate_tables(&request.queries, resolver).await?;

    // 3. Execute stages
    let all_results = execute_plan(&plan, &request.queries, resolver, admin).await?;

    // 4. Filter results for response
    let results = filter_results(&all_results, request, &plan);

    let elapsed = start.elapsed();

    Ok(BatchResponse {
        id: request.id.clone(),
        results,
        execution_plan: plan.stages.clone(),
        execution_time_us: elapsed.as_micros() as u64,
        transaction: None,
    })
}

/// Execute a batch request with permission checks.
///
/// Same as [`execute_batch`] but runs `SessionPermissions::check_batch`
/// before planning/execution. Returns `BatchError::QueryError` if any
/// operation is denied.
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
            message: format!(
                "Permission denied: {:?} on {:?}",
                action, resource
            ),
        })?;

    execute_batch(request, resolver, admin).await
}

/// Validate that all referenced tables exist before execution.
///
/// Fails fast with a clear error if any table is not found, rather than
/// discovering it mid-execution after some operations have already run.
async fn validate_tables(
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
) -> Result<(), BatchError> {
    // Collect unique table refs (skip admin ops which don't reference tables)
    let mut seen = shamir_types::types::common::new_set::<String>();
    for (alias, entry) in queries {
        if let Some(table_ref) = entry.op.table_ref() {
            let key = format!("{}/{}", table_ref.repo, table_ref.table);
            if seen.insert(key) {
                resolver.resolve(table_ref).await.map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.clone(),
                        message: format!(
                            "Table '{}' in repo '{}' not found: {}",
                            table_ref.table, table_ref.repo, e
                        ),
                    }
                })?;
            }
        }
    }
    Ok(())
}

/// Execute a planned batch stage by stage.
///
/// For each stage, executes all queries (sequentially within a stage for now).
/// Each query's FilterContext gets only the resolved_refs from its declared
/// dependencies — not all accumulated results.
async fn execute_plan(
    plan: &BatchPlan,
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
) -> Result<TMap<String, QueryResult>, BatchError> {
    let mut all_results: TMap<String, QueryResult> = new_map();

    for stage in &plan.stages {
        for alias in stage {
            let entry = queries.get(alias).ok_or_else(|| BatchError::QueryError {
                alias: alias.clone(),
                message: "Query entry not found".to_string(),
            })?;

            // Build resolved_refs with ONLY declared dependencies
            let deps = plan.dependencies.get(alias);
            let resolved_refs = build_resolved_refs(&all_results, deps);

            let result = execute_single(alias, entry, resolver, admin, &resolved_refs).await?;
            all_results.insert(alias.clone(), result);
        }
    }

    Ok(all_results)
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

/// Execute a single query/operation entry.
async fn execute_single(
    alias: &str,
    entry: &QueryEntry,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    resolved_refs: &TMap<String, QueryResult>,
) -> Result<QueryResult, BatchError> {
    // Admin ops — delegate to AdminExecutor
    if entry.op.is_admin() {
        return match admin {
            Some(executor) => executor.execute_admin(&entry.op).await,
            None => Err(BatchError::QueryError {
                alias: alias.to_string(),
                message: "Admin operations not supported in this context".to_string(),
            }),
        };
    }

    let table_ref = entry.op.table_ref().unwrap();
    let table = resolver
        .resolve(table_ref)
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: e.to_string(),
        })?;

    let interner = table
        .interner()
        .get()
        .await
        .map_err(|e| BatchError::QueryError {
            alias: alias.to_string(),
            message: e.to_string(),
        })?;

    let ctx = FilterContext::new(interner, resolved_refs);

    match &entry.op {
        BatchOp::Read(query) => table.read(query, &ctx).await.map_err(|e| {
            BatchError::QueryError {
                alias: alias.to_string(),
                message: e.to_string(),
            }
        }),

        BatchOp::Insert(op) => {
            let wr = table.execute_insert(op).await.map_err(|e| {
                BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(write_result_to_query_result(wr))
        }

        BatchOp::Update(op) => {
            let wr = table.execute_update(op, &ctx).await.map_err(|e| {
                BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(write_result_to_query_result(wr))
        }

        BatchOp::Delete(op) => {
            let wr = table.execute_delete(op, &ctx).await.map_err(|e| {
                BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(write_result_to_query_result(wr))
        }

        BatchOp::Set(op) => {
            let wr = table.execute_set(op).await.map_err(|e| {
                BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(write_result_to_query_result(wr))
        }

        // Admin ops are handled before this match via is_admin() check
        _ => unreachable!("Admin ops should have been handled earlier"),
    }
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
    }
}

/// Filter results based on return_all / return_only / return_result flags.
fn filter_results(
    all_results: &TMap<String, QueryResult>,
    request: &BatchRequest,
    _plan: &BatchPlan,
) -> TMap<String, QueryResult> {
    let mut out: TMap<String, QueryResult> = new_map();

    // return_only takes precedence
    if let Some(ref only) = request.return_only {
        for alias in only {
            if let Some(result) = all_results.get(alias) {
                out.insert(alias.clone(), result.clone());
            }
        }
        return out;
    }

    // Otherwise use return_all + per-entry return_result
    for (alias, result) in all_results {
        if !request.return_all {
            // Only include entries that explicitly set return_result = true
            if let Some(entry) = request.queries.get(alias) {
                if !entry.return_result {
                    continue;
                }
            }
        }
        out.insert(alias.clone(), result.clone());
    }

    out
}
