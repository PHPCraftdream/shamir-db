use shamir_collections::TFxSet;
use shamir_query_types::batch::ResultEncoding;
use shamir_query_types::filter::Filter;
use shamir_types::access::{Actor, ResourcePath};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::query::batch::exec_scope::ExecScope;
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;
use crate::query::TableRef;

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
/// `docs/dev-artifacts/pre-transactional/05-executor-isolation.md`.
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
    pub params: &'a TMap<String, QueryValue>,
    /// Result row encoding requested by the client. `Name` (default) =
    /// server de-interns to name-keyed `QueryValue`; `Id` = server
    /// returns raw id-keyed storage msgpack (client de-interns).
    pub result_encoding: ResultEncoding,
    /// Shared execution budget, consulted at the existing cooperative
    /// checkpoints and borrowed by every nested `Batch`/`ForEach` body.
    /// The borrow is bounded by this runner's execution lifetime.
    pub scope: &'a ExecScope,
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
    /// Each data op calls [`trace_access`] with the appropriate [`Action`]
    /// before performing work — an R2 OBSERVABILITY trace, always `Ok`,
    /// NOT the enforcement gate. The real gate already ran earlier, in
    /// `ShamirDb::execute_as` / `tx_execute_as` (the per-op
    /// `authorize_access` loop driven by `BatchOp::required_access`); by
    /// the time control reaches this `run` method the op has already been
    /// authorized. Do not remove the outer `authorize_access` call under
    /// the impression that the `trace_access` calls below are doing the
    /// enforcement — they never have.
    pub(in crate::query::batch) async fn run_with_wait(
        &mut self,
        alias: &str,
        entry: &QueryEntry,
        resolved_refs: &TMap<String, QueryResult>,
        wait: Option<shamir_query_types::batch::VisibilityWait>,
    ) -> Result<QueryResult, BatchError> {
        if let Some(position) = crate::query::batch::receipts::required(entry) {
            return crate::query::batch::receipts::failure_result(position);
        }
        let inherited = crate::query::batch::receipts::inherit(entry, wait);
        let entry = inherited.as_ref();
        // Sub-batch — recurse rather than delegating to AdminExecutor.
        if let BatchOp::Batch(_) = &entry.op {
            return self
                .run_sub_batch_arm(alias, entry, resolved_refs, wait)
                .await;
        }

        // ForEach loop (Epic04/B, #653) — recursive K-times execution,
        // reusing the exact recursive seam BatchOp::Batch uses above,
        // invoked in a loop instead of once. See
        // `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`.
        if let BatchOp::ForEach(_) = &entry.op {
            return self
                .run_for_each_arm(alias, entry, resolved_refs, wait)
                .await;
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
            // A Call delegates to FunctionInvoker with autocommit semantics —
            // the function's own writes commit independently of any outer
            // transaction. Inside a transactional batch (self.tx.is_some()
            // because execute_transactional_impl opened a TxContext) or an
            // interactive tx (self.tx.is_some() because execute_in_open_tx
            // threaded the caller's TxContext), that breaks atomicity
            // silently: the outer abort would not roll back the Call's writes.
            // Reject explicitly instead, mirroring the nested_tx_not_supported
            // guard for transactional sub-batches above.
            if self.tx.is_some() {
                return Err(BatchError::query_coded(
                    alias,
                    "call_in_tx_not_supported",
                    "a Call operation cannot run inside a transaction \
                     (its writes would commit independently of the outer transaction)",
                ));
            }
            return match self.invoker {
                Some(inv) => inv.invoke_call(call_op, &self.actor, resolved_refs).await,
                None => Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message: "Function calls not supported in this context".to_string(),
                    code: None,
                }),
            };
        }

        // Subscribe — validate sources exist, return a grant marker.
        // Real activation happens in the server handler post-processing.
        if let BatchOp::Subscribe(op) = &entry.op {
            let unique_repos: TFxSet<&str> =
                op.subscribe.iter().map(|s| s.table.repo.as_str()).collect();
            if unique_repos.len() > 1 {
                return Err(BatchError::QueryError {
                    alias: alias.to_string(),
                    message: "multi-repo subscriptions not yet supported".to_string(),
                    code: Some("multi_repo_subscriptions_not_supported".to_string()),
                });
            }

            for src in &op.subscribe {
                self.resolver
                    .resolve(&src.table)
                    .await
                    .map_err(|_| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("table not found: {}", src.table),
                        code: Some("table_not_found".to_string()),
                    })?;

                if let Some(ref filter) = src.filter {
                    if let Some(unsupported) = find_unsupported_subscription_filter(filter) {
                        return Err(BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "subscription filter uses unsupported operator: {unsupported}"
                            ),
                            code: Some("subscription_filter_unsupported_operator".to_string()),
                        });
                    }
                }
            }

            let mut grant_map = new_map();
            grant_map.insert("subscription_grant".to_string(), QueryValue::Bool(true));
            grant_map.insert(
                "sources_count".to_string(),
                QueryValue::Int(op.subscribe.len() as i64),
            );
            return Ok(QueryResult::with_value(QueryValue::Map(grant_map)));
        }

        // Unsubscribe — return a grant marker; real deactivation is server-side.
        if let BatchOp::Unsubscribe(op) = &entry.op {
            let mut grant_map = new_map();
            grant_map.insert("unsubscribe_grant".to_string(), QueryValue::Bool(true));
            grant_map.insert("sub_id".to_string(), QueryValue::Int(op.unsubscribe as i64));
            return Ok(QueryResult::with_value(QueryValue::Map(grant_map)));
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
            .with_max_accumulator_bytes(self.scope.limits().max_result_size)
            .with_scalars(self.resolver.scalar_resolver())
            .with_params(self.params);

        match &entry.op {
            BatchOp::Read(_) => {
                self.run_read_arm(
                    alias, entry, table_ref, resource, &table, interner, ctx, wait,
                )
                .await
            }
            BatchOp::Insert(_) => {
                self.run_insert_arm(
                    alias, entry, table_ref, resource, &table, interner, ctx, wait,
                )
                .await
            }
            BatchOp::Update(_) => {
                self.run_update_arm(
                    alias, entry, table_ref, resource, &table, interner, ctx, wait,
                )
                .await
            }
            BatchOp::Delete(_) => {
                self.run_delete_arm(
                    alias, entry, table_ref, resource, &table, interner, ctx, wait,
                )
                .await
            }
            BatchOp::Set(_) => {
                self.run_set_arm(
                    alias, entry, table_ref, resource, &table, interner, ctx, wait,
                )
                .await
            }

            // Admin ops are handled before this match via is_admin() check
            _ => unreachable!("Admin ops should have been handled earlier"),
        }
    }
}

/// Execute a single query/operation entry.
///
/// Thin wrapper around [`QueryRunner`] with `tx: None`.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::query::batch) async fn execute_single_impl(
    alias: &str,
    entry: &QueryEntry,
    resolver: &dyn TableResolver,
    admin: Option<&dyn AdminExecutor>,
    invoker: Option<&dyn FunctionInvoker>,
    resolved_refs: &TMap<String, QueryResult>,
    actor: &Actor,
    db_name: &str,
    depth: usize,
    params: &TMap<String, QueryValue>,
    result_encoding: ResultEncoding,
    scope: &ExecScope,
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
        result_encoding,
        scope,
    };
    runner.run(alias, entry, resolved_refs).await
}

/// Recursively walks a filter tree and returns `Some("variant_name")` for the
/// first variant that is NOT supported by the live subscription bridge evaluator.
///
/// Supported: Eq, Ne, Gt, Gte, Lt, Lte, In, NotIn, IsNull, IsNotNull,
/// Exists, NotExists, And, Or, Not.
fn find_unsupported_subscription_filter(filter: &Filter) -> Option<&'static str> {
    match filter {
        Filter::Eq { .. }
        | Filter::Ne { .. }
        | Filter::Gt { .. }
        | Filter::Gte { .. }
        | Filter::Lt { .. }
        | Filter::Lte { .. }
        | Filter::In { .. }
        | Filter::NotIn { .. }
        | Filter::IsNull { .. }
        | Filter::IsNotNull { .. }
        | Filter::Exists { .. }
        | Filter::NotExists { .. } => None,
        Filter::And { filters } => filters
            .iter()
            .find_map(find_unsupported_subscription_filter),
        Filter::Or { filters } => filters
            .iter()
            .find_map(find_unsupported_subscription_filter),
        Filter::Not { filter: f } => find_unsupported_subscription_filter(f),
        Filter::Like { .. } => Some("like"),
        Filter::ILike { .. } => Some("ilike"),
        Filter::Regex { .. } => Some("regex"),
        Filter::Contains { .. } => Some("contains"),
        Filter::ContainsAny { .. } => Some("contains_any"),
        Filter::ContainsAll { .. } => Some("contains_all"),
        Filter::Between { .. } => Some("between"),
        Filter::FieldEq { .. } => Some("field_eq"),
        Filter::Fts { .. } => Some("fts"),
        Filter::VectorSimilarity { .. } => Some("vector_similarity"),
        Filter::Computed { .. } => Some("computed"),
        // Value-vs-value comparison has no field/record involvement — not
        // a per-row predicate the live subscription bridge evaluator can
        // apply against a row's field data.
        Filter::ValueCompare { .. } => Some("value_compare"),
    }
}
