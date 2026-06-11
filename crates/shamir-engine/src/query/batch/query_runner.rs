//! Batch query executor — `QueryRunner` and supporting free functions.
//!
//! Executes a BatchPlan stage by stage, passing results between
//! dependent queries via FilterContext::resolved_refs.

use crate::query::batch::batch_execute::execute_batch_impl;
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use crate::query::filter::FilterContext;
use crate::query::read::{QueryResult, QueryStats};
use crate::query::write::WriteResult;
use crate::query::TableRef;
use shamir_types::access::{authorize, Action, Actor, ResourcePath};
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::InnerValue;

use crate::query::batch::param_subst::substitute_params;
use shamir_query_types::filter::Filter;

/// Build resolved_refs map containing only the declared dependencies.
pub(super) fn build_resolved_refs(
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

        // Subscribe — validate sources exist, return a grant marker.
        // Real activation happens in the server handler post-processing.
        if let BatchOp::Subscribe(op) = &entry.op {
            let unique_repos: std::collections::HashSet<&str> =
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

            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: Some(serde_json::json!({
                    "subscription_grant": true,
                    "sources_count": op.subscribe.len()
                })),
            });
        }

        // Unsubscribe — return a grant marker; real deactivation is server-side.
        if let BatchOp::Unsubscribe(op) = &entry.op {
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: Some(serde_json::json!({
                    "unsubscribe_grant": true,
                    "sub_id": op.unsubscribe
                })),
            });
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
                // Substitute $param references inside each inserted value row.
                // substitute_params has a fast path (clone) for rows with no $param nodes.
                let subst_values: Result<Vec<_>, _> = op
                    .values
                    .iter()
                    .map(|v| {
                        substitute_params(v, self.params).map_err(|name| {
                            BatchError::query_coded(
                                alias,
                                "unbound_param",
                                format!("$param '{}' is not bound in this sub-batch", name),
                            )
                        })
                    })
                    .collect();
                let subst_op;
                let op_ref: &shamir_query_types::write::InsertOp = match subst_values {
                    Ok(values) if values == op.values => op,
                    Ok(values) => {
                        subst_op = shamir_query_types::write::InsertOp {
                            insert_into: op.insert_into.clone(),
                            values,
                        };
                        &subst_op
                    }
                    Err(e) => return Err(e),
                };
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_insert_tx(op_ref, tx).await,
                    None => table.execute_insert(op_ref).await,
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
                // Substitute $param references inside the `set` document.
                // substitute_params has a fast path (clone) when no $param nodes exist.
                let subst_set = substitute_params(&op.set, self.params).map_err(|name| {
                    BatchError::query_coded(
                        alias,
                        "unbound_param",
                        format!("$param '{}' is not bound in this sub-batch", name),
                    )
                })?;
                let subst_op;
                let op_ref: &shamir_query_types::write::UpdateOp = if subst_set == op.set {
                    op
                } else {
                    subst_op = shamir_query_types::write::UpdateOp {
                        update: op.update.clone(),
                        where_clause: op.where_clause.clone(),
                        set: subst_set,
                        select: op.select.clone(),
                    };
                    &subst_op
                };
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_update_tx(op_ref, &ctx, tx).await,
                    None => table.execute_update(op_ref, &ctx).await,
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
                // Substitute $param references inside the upsert `key` and `value`.
                // substitute_params has a fast path (clone) when no $param nodes exist.
                let subst_key = substitute_params(&op.key, self.params).map_err(|name| {
                    BatchError::query_coded(
                        alias,
                        "unbound_param",
                        format!("$param '{}' is not bound in this sub-batch", name),
                    )
                })?;
                let subst_value = substitute_params(&op.value, self.params).map_err(|name| {
                    BatchError::query_coded(
                        alias,
                        "unbound_param",
                        format!("$param '{}' is not bound in this sub-batch", name),
                    )
                })?;
                let subst_op;
                let op_ref: &shamir_query_types::write::SetOp =
                    if subst_key == op.key && subst_value == op.value {
                        op
                    } else {
                        subst_op = shamir_query_types::write::SetOp {
                            set: op.set.clone(),
                            key: subst_key,
                            value: subst_value,
                        };
                        &subst_op
                    };
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table.execute_set_tx(op_ref, tx).await,
                    None => table.execute_set(op_ref).await,
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
pub(super) async fn execute_single_impl(
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
pub(super) fn write_result_to_query_result(wr: WriteResult) -> QueryResult {
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
    }
}

// Re-export public items used outside this module
pub use crate::query::batch::batch_execute::execute_batch;
#[cfg(test)]
pub use crate::query::batch::batch_execute::execute_batch_with_permissions;
pub use crate::query::batch::interactive_tx::{
    commit_interactive_tx, execute_in_open_tx, open_interactive_tx,
};
