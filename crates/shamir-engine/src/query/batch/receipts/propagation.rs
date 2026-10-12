use super::accounting::write_result_to_query_result_with_encoding;
use crate::query::batch::exec_scope::ExecScope;
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::query_runner::{build_resolved_refs, resolve_skip, skipped_query_result};
use crate::query::batch::result_size::ResultSize;
use crate::query::batch::{BatchError, BatchOp, BatchPlan, BatchRequest, QueryEntry, QueryRunner};
use crate::query::read::QueryResult;
use crate::query::write::WriteResult;
use shamir_query_types::batch::{ResultEncoding, TransactionInfo, VisibilityWait};
use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map_wc, TMap};
use shamir_types::types::value::QueryValue;

impl QueryRunner<'_> {
    /// Value and receipt-bearing execution with the repository's default policy.
    pub async fn run(
        &mut self,
        alias: &str,
        entry: &QueryEntry,
        refs: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        // Boxed: the per-op future is large and must not be copied by value
        // through the task-local wrapper at every recursion level.
        super::dependency::scope(Box::pin(self.run_with_wait(alias, entry, refs, None))).await
    }
}

pub(in crate::query::batch) fn filter(
    mut results: TMap<String, QueryResult>,
    request: &BatchRequest,
) -> TMap<String, QueryResult> {
    for (alias, result) in &mut results {
        let selected = request.return_only.as_ref().map_or_else(
            || request.return_all || request.queries.get(alias).is_some_and(|e| e.return_result),
            |only| only.contains(alias),
        );
        if !selected && result.receipt.is_some() {
            result.records.clear();
            result.value = None;
            result.stats = None;
            result.pagination = None;
            result.versions = None;
        }
    }
    super::super::batch_execute::filter_results(results, request)
}

pub(in crate::query::batch) fn stamp(
    mut results: TMap<String, QueryResult>,
    queries: &TMap<String, QueryEntry>,
    info: &TransactionInfo,
    depth: usize,
) -> TMap<String, QueryResult> {
    if depth > 0 {
        for (alias, result) in &mut results {
            if !result.skipped && queries.get(alias).is_some_and(|e| e.op.is_write()) {
                if let Some(table) = queries.get(alias).and_then(|e| e.op.table_ref()) {
                    super::dependency::record(&table.repo, info);
                }
                result.receipt = Some(info.clone());
            }
        }
    }
    results
}

pub(in crate::query::batch) fn attach(
    value: WriteResult,
    encoding: ResultEncoding,
    interner: &Interner,
    receipt: TransactionInfo,
) -> QueryResult {
    QueryResult {
        receipt: Some(receipt),
        ..write_result_to_query_result_with_encoding(value, encoding, interner)
    }
}

/// Receipt-bearing non-transactional dispatch. Only pending receipts require
/// dependency tracking; Visible adds no map, timer, or atomic RMW.
#[allow(clippy::too_many_arguments)]
pub(in crate::query::batch) async fn execute_plan(
    plan: &mut BatchPlan,
    queries: &TMap<String, QueryEntry>,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    actor: &Actor,
    db_name: &str,
    depth: usize,
    params: &TMap<String, QueryValue>,
    encoding: ResultEncoding,
    scope: &ExecScope,
    wait: Option<VisibilityWait>,
) -> Result<TMap<String, QueryResult>, BatchError> {
    let mut results: TMap<String, QueryResult> = new_map_wc(queries.len());
    for stage in &plan.stages {
        for alias in stage {
            scope.check()?;
            let entry = queries.get(alias).ok_or_else(|| {
                BatchError::query_coded(alias, "validation", "query entry not found")
            })?;
            let deps = plan.edge_provenance.get(alias);
            let failed_dependency = deps.and_then(|deps| {
                deps.iter()
                    .filter(|(_, kind)| kind.is_data_flow())
                    .find_map(|(alias, _)| results.get(alias).and_then(super::dependency::failure))
            });
            let refs = build_resolved_refs(&results, deps);
            if let Some(position) = failed_dependency {
                let result = super::dependency::failure_result(position)?;
                if scope.enforce_result_size() {
                    scope.charge_bytes(ResultSize::result(&result))?;
                }
                results.insert(alias.clone(), result);
                continue;
            }
            if resolve_skip(
                entry,
                &results,
                deps,
                &refs,
                params,
                resolver.scalar_resolver(),
            ) {
                let result = skipped_query_result();
                if scope.enforce_result_size() {
                    scope.charge_bytes(ResultSize::result(&result))?;
                }
                results.insert(alias.clone(), result);
                continue;
            }
            // Caller-reconcile contract: a pending commit never provides a
            // visibility fence. Fail just this read; independent aliases continue.
            let write_dependency = if matches!(&entry.op, BatchOp::Read(_)) {
                deps.and_then(|deps| {
                    deps.keys()
                        .find_map(|alias| results.get(alias).and_then(super::dependency::pending))
                })
            } else {
                None
            };
            let pending = write_dependency.or_else(|| super::dependency::required(entry));
            if let Some((commit_version, visible_through)) = pending {
                let result = super::dependency::failure_result((commit_version, visible_through))?;
                if scope.enforce_result_size() {
                    scope.charge_bytes(ResultSize::result(&result))?;
                }
                results.insert(alias.clone(), result);
                continue;
            }
            scope.charge_work(1)?;
            let mut runner = QueryRunner {
                resolver,
                admin,
                invoker,
                tx: None,
                actor: actor.clone(),
                db_name,
                depth,
                params,
                result_encoding: encoding,
                scope,
            };
            let _guard = crate::query::batch::op_watchdog::register_op_watchdog(alias);
            let started = std::time::Instant::now();
            let result = runner.run_with_wait(alias, entry, &refs, wait).await?;
            if started.elapsed() > std::time::Duration::from_secs(1) {
                log::warn!("batch op '{alias}' exceeded the one-second slow-op threshold");
            }
            if scope.enforce_result_size()
                && !matches!(
                    &entry.op,
                    BatchOp::Read(_) | BatchOp::Batch(_) | BatchOp::ForEach(_)
                )
            {
                scope.charge_bytes(ResultSize::result(&result))?;
            }
            if let (Some(table), Some(receipt)) = (entry.op.table_ref(), &result.receipt) {
                super::dependency::record(&table.repo, receipt);
            }
            results.insert(alias.clone(), result);
        }
    }
    Ok(results)
}
