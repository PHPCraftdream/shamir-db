//! Batch query executor — `QueryRunner` and supporting free functions.
//!
//! Executes a BatchPlan stage by stage, passing results between
//! dependent queries via FilterContext::resolved_refs.

use crate::query::batch::batch_execute::{execute_batch_impl, execute_plan_tx_impl};
use crate::query::batch::batch_validate::{validate_filter_depth, validate_tables};
use crate::query::batch::executor_traits::{AdminExecutor, FunctionInvoker, TableResolver};
use crate::query::batch::{BatchError, BatchOp, QueryEntry};
use crate::query::filter::FilterContext;
use crate::query::read::{QueryRecord, QueryResult, QueryStats};
use crate::query::write::WriteResult;
use crate::query::TableRef;
use serde_bytes::ByteBuf;
use shamir_collections::TFxSet;
use shamir_query_types::batch::{BatchLimits, ResultEncoding};
use shamir_types::access::{trace_access, Action, Actor, ResourcePath};
use shamir_types::codecs::interned::query_value_to_storage_bytes_into;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::{new_map, new_map_wc, TMap};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::param_subst::{contains_param_ref, resolve_write_value, WriteValueError};
use shamir_query_types::filter::Filter;

/// Absolute, server-enforced ceiling on `ForEach` iteration count —
/// independent of whatever a client-supplied `BatchLimits.max_iterations`
/// claims. Closes a DoS gap: `max_iterations` is entirely client-supplied
/// wire data (#666), so a client could otherwise set it to `usize::MAX`
/// and defeat the gate for a `ForEach` with a dynamic (non-literal-array)
/// `over` source, whose iteration count the plan-time `virtual_units`
/// check (`BatchPlanner::plan`) cannot see ahead of time.
pub(crate) const ABSOLUTE_MAX_FOR_EACH_ITERATIONS: usize = 100_000;

/// The effective `max_iterations` gate value for a `ForEach` body's
/// `limits` — the client-supplied value, clamped down to the server's
/// absolute ceiling (`ABSOLUTE_MAX_FOR_EACH_ITERATIONS`). A `min`, never a
/// replacement: a client-supplied value BELOW the ceiling is respected
/// unchanged (#653 behavior preserved), only a value AT OR ABOVE the
/// ceiling is clamped down (#666).
pub(crate) fn effective_max_iterations(limits: &BatchLimits) -> usize {
    limits.max_iterations.min(ABSOLUTE_MAX_FOR_EACH_ITERATIONS)
}

/// Build resolved_refs map containing only DataFlow-provenance dependencies.
///
/// `after`-only edges (`EdgeKind::Explicit`) are ordering hints, NOT data
/// access grants — they must NOT leak the referenced alias's result into
/// the dependent op's `FilterContext`. Only edges with a real `$query`
/// reference (`EdgeKind::DataFlow` or `EdgeKind::Both`) resolve to data.
pub(super) fn build_resolved_refs(
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
) -> TMap<String, QueryResult> {
    let cap = provenance.map_or(0, |p| p.len());
    let mut refs = new_map_wc(cap);
    if let Some(provenance) = provenance {
        for (dep_alias, kind) in provenance {
            if !kind.is_data_flow() {
                continue;
            }
            if let Some(result) = all_results.get(dep_alias) {
                refs.insert(dep_alias.clone(), result.clone());
            }
        }
    }
    refs
}

/// Build the "did not run" [`QueryResult`] for a skipped alias (Epic03/B,
/// #645) — own `when` evaluated `false`, or cascaded from a skipped
/// `DataFlow`/`Both`-provenance dependency (ADR Decision 2/4). Empty
/// records/stats/pagination/value/explain, `skipped: true` — recognizably
/// empty-but-explicit, distinct from "0 rows matched" (`skipped: false`,
/// `stats: Some(..)`) and from "filtered by `return_only`" (alias absent
/// from `results` entirely, unrelated to this flag).
pub(super) fn skipped_query_result() -> QueryResult {
    QueryResult {
        records: Vec::new(),
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: true,
    }
}

/// True if `alias`'s dependencies (per `provenance`) contain at least one
/// `DataFlow`/`Both`-provenance edge onto an already-skipped result in
/// `all_results`.
///
/// `Explicit`-only (`after`) edges do NOT cascade — pure ordering, no data
/// access, per Epic01/A's already-shipped invariant (see
/// `build_resolved_refs`'s doc comment above). Only edges that actually
/// grant data access propagate a skip: if `B` never observes `A`'s result
/// (a plain `after` edge), `B` running or not running is independent of
/// whether `A` ran.
fn has_skipped_data_flow_dep(
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
) -> bool {
    let Some(provenance) = provenance else {
        return false;
    };
    provenance.iter().any(|(dep_alias, kind)| {
        kind.is_data_flow() && all_results.get(dep_alias).is_some_and(|r| r.skipped)
    })
}

/// Decide whether `entry` should be skipped for this stage, and return the
/// skip result if so.
///
/// Precedence (documented decision — the ADR does not state this
/// explicitly, so Phase B fixes it here): **cascade wins over the entry's
/// own `when`.** If a real data dependency was skipped, the current op's
/// inputs are already missing/undefined (a `$query` ref onto a skipped
/// alias would resolve to nothing) — evaluating the op's own `when` against
/// an incomplete `resolved_refs` would be evaluating a condition over data
/// that was never produced, not a meaningful "did the branch's guard hold"
/// check. So cascade is checked FIRST and short-circuits before compiling
/// or evaluating `entry.when` at all.
pub(super) fn resolve_skip(
    entry: &QueryEntry,
    all_results: &TMap<String, QueryResult>,
    provenance: Option<&TMap<String, shamir_query_types::batch::EdgeKind>>,
    resolved_refs: &TMap<String, QueryResult>,
    actor: &Actor,
    params: &TMap<String, QueryValue>,
) -> bool {
    // Cascade: a genuine (DataFlow/Both) dependency was itself skipped.
    // Takes precedence over the entry's own `when` — see this function's
    // doc comment above for the rationale (undocumented in the ADR;
    // decided here since a skipped dependency means this op's inputs are
    // already missing, so evaluating its own `when` would be evaluating a
    // condition over data that was never produced).
    if has_skipped_data_flow_dep(all_results, provenance) {
        return true;
    }

    let Some(filter) = &entry.when else {
        return false;
    };

    // Evaluate `when` against an empty synthetic record via a scratch
    // interner — `when` has no per-row context (ADR Decision 1): only
    // `$query`/`$fn`/`$param`/literals are meaningful inside a `when`
    // filter (no `FieldRef` support needed/meaningful), and
    // `InnerValue::Null`'s `RecordRef` impl is exactly the "no field data"
    // record already used for sub-batch `bind` resolution (see
    // `QueryRunner::run`'s `dummy_record` below). A fresh `Interner::new()`
    // is correct here (not the table's real interner) for the same reason
    // the bind-resolution path uses one: any field path in `when` folds to
    // `FilterNode::True`/`False` regardless of which interner is used,
    // since no field ever actually resolves against a real record.
    let scratch = shamir_types::core::interner::Interner::new();
    let ctx = FilterContext::new(&scratch, resolved_refs)
        .with_actor(actor.clone())
        .with_params(params);
    let node = crate::query::filter::compile_filter(filter, &scratch);
    !node.matches(&InnerValue::Null, &ctx)
}

/// Run a nested `Batch`/`ForEach` body (which is NOT itself marked
/// `transactional` — the only reachable case once the `nested_tx_not_supported`
/// guard has already rejected `body.transactional && tx.is_some()`) so its
/// writes are genuine participants in the ALREADY-OPEN outer transaction
/// `tx`, instead of each write silently committing independently via
/// `execute_batch_impl`'s non-transactional (implicit-per-op-tx) path (#661).
///
/// This replicates the setup steps `execute_batch_impl` performs internally
/// (plan via `BatchPlanner::plan`, `validate_tables`, `validate_filter_depth`)
/// and then drives the plan directly through [`execute_plan_tx_impl`],
/// reusing `tx` — so a failure anywhere in the nested body propagates as an
/// `Err` out of the SAME `TxContext` the outer `execute_transactional_impl`
/// already knows how to roll back on `Err` (RAII: the tx is dropped without
/// commit), with zero changes needed to that commit/abort logic.
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
fn run_nested_body_in_outer_tx<'a>(
    body: &'a crate::query::batch::BatchRequest,
    resolver: &'a dyn TableResolver,
    admin: Option<&'a dyn AdminExecutor>,
    invoker: Option<&'a dyn FunctionInvoker>,
    actor: &'a Actor,
    db_name: &'a str,
    tx: &'a mut shamir_tx::TxContext,
    depth: usize,
    params: &'a TMap<String, QueryValue>,
    result_encoding: ResultEncoding,
) -> NestedTxBodyFuture<'a> {
    Box::pin(async move {
        let mut plan = shamir_query_types::batch::BatchPlanner::plan(&body.queries, &body.limits)?;
        validate_tables(&body.queries, resolver).await?;
        validate_filter_depth(&body.queries)?;

        execute_plan_tx_impl(
            &mut plan,
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
        )
        .await
    })
}

/// Map a [`WriteValueError`] (from [`resolve_write_value`]) to the
/// [`BatchError`] shape every Insert/Update/Set dispatch arm already returns
/// for its `$param`-only substitution failures — `unbound_param` keeps its
/// pre-existing code, `malformed_marker` is new (a `$query`/`$fn`/`$cond`/
/// `$expr` marker that failed to resolve).
fn write_value_error_to_batch_error(alias: &str, err: WriteValueError) -> BatchError {
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
            // and params. Each value is a FilterValue — resolve it to a
            // QueryValue using the same machinery as filter evaluation.
            // We use a dummy record (Null) because bind values may only
            // reference $query aliases or literals, not record fields.
            let dummy_record = InnerValue::Null;
            // We need an Interner for FilterContext, but bind values must only
            // be literals or $query refs (not FieldRefs). Use a scratch interner.
            let scratch = shamir_types::core::interner::Interner::new();
            let bind_ctx = FilterContext::new(&scratch, resolved_refs)
                .with_actor(self.actor.clone())
                .with_params(self.params);
            let mut resolved_params: TMap<String, QueryValue> = new_map();
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
                        let v = crate::query::filter::eval::resolve_filter_query(
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

            // Recurse into the sub-batch. When we're already inside an
            // open outer transaction (`self.tx.is_some()`), thread that
            // SAME `TxContext` through so the sub-batch's writes are
            // genuine participants in the outer tx — an error anywhere in
            // the sub-batch then aborts the WHOLE outer transaction via the
            // outer `execute_transactional_impl`'s existing RAII rollback
            // (#661). The `nested_tx_not_supported` guard above already
            // ensures `sub.batch.transactional == false` is the only
            // reachable case here. When there is no open outer tx
            // (`self.tx.is_none()`), keep the original `execute_batch_impl`
            // path byte-identical.
            let inner_results = match self.tx.as_deref_mut() {
                Some(tx) => run_nested_body_in_outer_tx(
                    &sub.batch,
                    self.resolver,
                    self.admin,
                    self.invoker,
                    &self.actor,
                    self.db_name,
                    tx,
                    self.depth + 1,
                    &resolved_params,
                    self.result_encoding,
                )
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: format!("sub-batch '{}' failed: {}", alias, e),
                    code: e.code().map(str::to_owned),
                })?,
                None => {
                    execute_batch_impl(
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
                    })?
                    .results
                }
            };

            // Wrap the inner results into a QueryResult for the outer
            // $query path resolution.
            //
            // `resolve_query_ref_value` in eval.rs checks `qr.value` first
            // (Call-result path). We store the inner results map as a QueryValue
            // Map in `value` so outer ops can access sub-aliases:
            //   $query @sub[0].records[0].id  — NOT supported (records empty)
            //   $query @sub.alias_name[0].id  — walks value.alias_name[0].id
            //
            // Round-trip via msgpack: QueryResult's Serialize is well-defined
            // and produces the same wire shape as QueryValue's Deserialize expects.
            let value = rmp_serde::to_vec_named(&inner_results)
                .ok()
                .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok());
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value,
                explain: None,
                skipped: false,
            });
        }

        // ForEach loop (Epic04/B, #653) — recursive K-times execution,
        // reusing the exact recursive seam BatchOp::Batch uses above,
        // invoked in a loop instead of once. See
        // `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`.
        if let BatchOp::ForEach(fe) = &entry.op {
            // Guard: transactional loop body inside an already-open tx is
            // not supported — identical rationale to the sub-batch guard
            // above (two-phase commit across a shared TxContext is unsafe).
            if fe.batch.transactional && self.tx.is_some() {
                return Err(BatchError::query_coded(
                    alias,
                    "nested_tx_not_supported",
                    "a transactional for_each body cannot run inside an outer transaction",
                ));
            }

            // Resolve `over` against the CURRENT scope's resolved_refs and
            // params, exactly like `SubBatchOp.bind`'s resolution above:
            // a scratch Interner is safe here because `over` (like `bind`
            // values) may only reference $query aliases, $param, or
            // literals — never a record FieldRef (see ADR's bug #651
            // independence analysis).
            let dummy_record = InnerValue::Null;
            let scratch = shamir_types::core::interner::Interner::new();
            let over_ctx = FilterContext::new(&scratch, resolved_refs)
                .with_actor(self.actor.clone())
                .with_params(self.params);

            // Column-ref form (`@alias[].field`) needs the "whole column"
            // resolver (`resolve_query_ref_column`), not the scalar
            // `resolve_filter_query` — a bare `resolve_filter_query` call on
            // a `[]`-prefixed QueryRef path does not walk every record, only
            // `resolve_query_ref_column` does (see Decision 1's discussion
            // of `@alias[].field`). Every other `over` shape (literal
            // array, `$param`, nested `$cond`/`$expr`) resolves through the
            // normal scalar path and is expected to yield a
            // `QueryValue::List`.
            let elements: Vec<QueryValue> = if let crate::query::filter::FilterValue::QueryRef {
                alias: over_alias,
                path,
            } = &fe.over
            {
                if crate::query::filter::is_column_query_ref(&fe.over) {
                    let key = over_alias.strip_prefix('@').unwrap_or(over_alias.as_str());
                    match resolved_refs.get(key) {
                        Some(qr) => {
                            crate::query::filter::resolve_query_ref_column(qr, path.as_deref())
                        }
                        None => Vec::new(),
                    }
                } else {
                    match crate::query::filter::eval::resolve_filter_query(
                        &fe.over,
                        &dummy_record,
                        &over_ctx,
                    ) {
                        Some(QueryValue::List(items)) => items,
                        Some(_) | None => {
                            return Err(BatchError::QueryError {
                                alias: alias.to_string(),
                                message: format!(
                                    "for_each '{}': 'over' did not resolve to a list",
                                    alias
                                ),
                                code: None,
                            });
                        }
                    }
                }
            } else {
                match crate::query::filter::eval::resolve_filter_query(
                    &fe.over,
                    &dummy_record,
                    &over_ctx,
                ) {
                    Some(QueryValue::List(items)) => items,
                    Some(_) | None => {
                        return Err(BatchError::QueryError {
                            alias: alias.to_string(),
                            message: format!(
                                "for_each '{}': 'over' did not resolve to a list",
                                alias
                            ),
                            code: None,
                        });
                    }
                }
            };

            // Runtime gate (ADR Decision 3): reject BEFORE iteration 0 if
            // the resolved length exceeds max_iterations — never a partial
            // run followed by a mid-loop abort.
            let max_iterations = effective_max_iterations(&fe.batch.limits);
            if elements.len() > max_iterations {
                return Err(BatchError::TooManyIterations {
                    alias: alias.to_string(),
                    actual: elements.len(),
                    max: max_iterations,
                });
            }

            let mut iterations: Vec<QueryValue> = Vec::with_capacity(elements.len());
            for element in elements {
                let mut resolved_params: TMap<String, QueryValue> = new_map();
                resolved_params.insert(fe.bind_row.clone(), element);

                // Recurse into the loop body — same seam as BatchOp::Batch,
                // invoked in a loop instead of once (ADR Decision 1).
                // Error semantics (ADR Decision 4): the `?` below propagates
                // the FIRST error immediately — both the tx case (abort the
                // whole containing batch) and the non-tx case (stop-at-
                // first, no partial results collected) fall out of this
                // same short-circuit, exactly like the existing sub-batch
                // recursion and the executor's stage loop.
                //
                // When we're already inside an open outer transaction
                // (`self.tx.is_some()`), thread that SAME `TxContext`
                // through every iteration — a re-borrow via
                // `as_deref_mut()` fresh on each loop pass, since a `&mut`
                // reborrow cannot be reused by value across iterations —
                // so all iterations' writes are genuine participants in
                // the outer tx and a failure on ANY iteration aborts the
                // WHOLE outer transaction via the outer
                // `execute_transactional_impl`'s existing RAII rollback
                // (#661), rather than each already-succeeded iteration
                // having committed independently and survived the abort.
                // The `nested_tx_not_supported` guard above already ensures
                // `fe.batch.transactional == false` is the only reachable
                // case here. When there is no open outer tx
                // (`self.tx.is_none()`), keep the original
                // `execute_batch_impl` path byte-identical.
                let inner_results = match self.tx.as_deref_mut() {
                    Some(tx) => run_nested_body_in_outer_tx(
                        &fe.batch,
                        self.resolver,
                        self.admin,
                        self.invoker,
                        &self.actor,
                        self.db_name,
                        tx,
                        self.depth + 1,
                        &resolved_params,
                        self.result_encoding,
                    )
                    .await
                    .map_err(|e| BatchError::QueryError {
                        alias: alias.to_string(),
                        message: format!("for_each '{}' iteration failed: {}", alias, e),
                        code: e.code().map(str::to_owned),
                    })?,
                    None => {
                        execute_batch_impl(
                            &fe.batch,
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
                            message: format!("for_each '{}' iteration failed: {}", alias, e),
                            code: e.code().map(str::to_owned),
                        })?
                        .results
                    }
                };

                // Same per-iteration value shape a single sub-batch already
                // produces (round-trip via msgpack) — collected into a
                // List, one entry per iteration (ADR Decision 2).
                let value = rmp_serde::to_vec_named(&inner_results)
                    .ok()
                    .and_then(|b| rmp_serde::from_slice::<QueryValue>(&b).ok())
                    .unwrap_or(QueryValue::Null);
                iterations.push(value);
            }

            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: Some(QueryValue::List(iterations)),
                explain: None,
                skipped: false,
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
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: Some(QueryValue::Map(grant_map)),
                explain: None,
                skipped: false,
            });
        }

        // Unsubscribe — return a grant marker; real deactivation is server-side.
        if let BatchOp::Unsubscribe(op) = &entry.op {
            let mut grant_map = new_map();
            grant_map.insert("unsubscribe_grant".to_string(), QueryValue::Bool(true));
            grant_map.insert("sub_id".to_string(), QueryValue::Int(op.unsubscribe as i64));
            return Ok(QueryResult {
                records: Vec::new(),
                stats: None,
                pagination: None,
                value: Some(QueryValue::Map(grant_map)),
                explain: None,
                skipped: false,
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
            .with_scalars(self.resolver.scalar_resolver())
            .with_params(self.params);

        match &entry.op {
            BatchOp::Read(query) => {
                // Observability trace only — see `trace_access`'s doc
                // comment. Real enforcement already ran in execute_as /
                // tx_execute_as before this runner was reached.
                trace_access(&self.actor, &resource, Action::Read).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                // D2 P1d-2b: TEMPORAL reads (AsOf / History) scan the durable
                // `history` version-log, which the cutover made the background
                // drainer fill (the ack-path now writes only the in-memory
                // overlay). The standard `Latest` read is overlay-aware (P1b
                // seams) and needs no drain, but a temporal read issued right
                // after a commit could race the async drainer and miss the
                // freshly-committed tail. Drain the repo first so temporal
                // reads are coherent regardless of drainer timing. `drain_all`
                // is idempotent / cheap when caught up; skipped for `Latest`.
                if !matches!(query.temporal, shamir_query_types::read::Temporal::Latest) {
                    if let Ok(repo) = self.resolver.resolve_repo(&table_ref.repo).await {
                        if let Err(e) = repo.drainer().drain_all(&repo).await {
                            log::warn!("temporal read: drain_all {}: {e}", table_ref.repo);
                        }
                    }
                }
                // Vector I.1: in a transactional batch route the read through
                // `read_tx` with a SHARED `&TxContext` so the SELECT records
                // into the read-set (Serializable → SSI write-skew detection
                // goes live end-to-end). `as_deref()` reborrows the runner's
                // `&mut TxContext` as `&TxContext`; the read branch never also
                // holds the `&mut`, and queries within a stage run sequentially
                // (no read/write aliasing over the same tx). Non-tx batches
                // keep the original zero-overhead `read` path.
                //
                // S-read: thread result_encoding into the read path so that
                // Id-encoding requests return id-keyed IdBytes rows instead of
                // de-interning on the server.
                match self.tx.as_deref() {
                    Some(tx) => {
                        table
                            .read_tx_with_encoding(query, &ctx, Some(tx), self.result_encoding)
                            .await
                    }
                    None => {
                        table
                            .read_with_encoding(query, &ctx, self.result_encoding)
                            .await
                    }
                }
                .map_err(|e| BatchError::QueryError {
                    alias: alias.to_string(),
                    message: e.to_string(),
                    code: None,
                })
            }

            BatchOp::Insert(op) => {
                // Observability trace only — see `trace_access`'s doc
                // comment. Real enforcement already ran in execute_as /
                // tx_execute_as before this runner was reached.
                trace_access(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                // #641: resolve reserved markers ($param/$query/$fn/$cond/
                // $expr) inside each inserted value row — but ONLY when a row
                // actually contains a marker node.
                //
                // The common bulk-insert path carries no markers: a cheap scan
                // (`contains_param_ref`, no allocation) lets us borrow `op`
                // directly — skipping the per-row clone-Vec AND the O(N) deep
                // structural `values == op.values` compare that the old code
                // paid on every insert to discover "nothing changed". Only when
                // some row references a marker do we build the resolved op
                // (and pay the clone), where resolution is genuinely needed.
                let subst_op;
                let needs_subst = op.values.iter().any(contains_param_ref);
                let op_ref: &shamir_query_types::write::InsertOp = if !needs_subst {
                    op
                } else {
                    let values: Vec<_> = op
                        .values
                        .iter()
                        .map(|v| {
                            resolve_write_value(v, &ctx)
                                .map_err(|e| write_value_error_to_batch_error(alias, e))
                        })
                        .collect::<Result<_, _>>()?;
                    subst_op = shamir_query_types::write::InsertOp {
                        insert_into: op.insert_into.clone(),
                        values,
                        records_idmsgpack: Vec::new(),
                        select: op.select.clone(),
                    };
                    &subst_op
                };
                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => table
                        .execute_insert_tx(op_ref, tx, entry.return_result, Some(self.resolver))
                        .await
                        .map_err(|e| BatchError::QueryError {
                            alias: alias.to_string(),
                            message: e.to_string(),
                            code: None,
                        })?,
                    // F4b-1: "everything is a transaction" — a non-tx insert is
                    // routed through the tx commit pipeline as an implicit
                    // single-op BATCH transaction (Snapshot isolation, so SSI
                    // never aborts → preserves non-tx last-writer-wins). This
                    // folds data + index postings + counter into ONE
                    // `WalEntryV2` and consumes ONE commit_version for the whole
                    // batch, matching tx-batch semantics.
                    None => {
                        let repo =
                            self.resolver
                                .resolve_repo(&table_ref.repo)
                                .await
                                .map_err(|e| BatchError::QueryError {
                                    alias: alias.to_string(),
                                    message: format!("resolve_repo({}): {}", table_ref.repo, e),
                                    code: None,
                                })?;
                        let return_result = entry.return_result;
                        // Move owned copies into the staging closure so the
                        // staged future borrows ONLY the tx (the `for<'t>`
                        // HRTB requires no other caller-scope borrows).
                        let owned_op: shamir_query_types::write::InsertOp = op_ref.clone();
                        let owned_table = table.clone();
                        repo.run_implicit_batch_tx(self.actor.clone(), alias, move |tx| {
                            Box::pin(async move {
                                owned_table
                                    .execute_insert_tx(&owned_op, tx, return_result, None)
                                    .await
                            })
                        })
                        .await?
                    }
                };
                Ok(write_result_to_query_result_with_encoding(
                    wr,
                    self.result_encoding,
                    interner,
                ))
            }

            BatchOp::Update(op) => {
                // Observability trace only — see `trace_access`'s doc
                // comment. Real enforcement already ran in execute_as /
                // tx_execute_as before this runner was reached.
                trace_access(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                // #641: resolve reserved markers ($param/$query/$fn/$cond/
                // $expr) inside the `set` document. `resolve_write_value` has
                // a fast path (clone) when no marker nodes exist.
                let subst_set = resolve_write_value(&op.set, &ctx)
                    .map_err(|e| write_value_error_to_batch_error(alias, e))?;
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

                // Phase ②.2b — ON UPDATE referential-action enforcement.
                //
                // Mirrors the delete-path (Phase D.1/D.2): build the fan-out
                // plan BEFORE the tx closure (the resolver cannot be captured
                // into the HRTB closure of `run_implicit_batch_tx` — same
                // constraint as RESTRICT/CASCADE on delete). The plan carries
                // pre-resolved child `TableManager` handles + the old→new
                // value pairs; an empty plan (the common case — update does
                // not touch a referenced parent field) is a fast no-op.
                //
                // TOCTOU caveat: same as delete — see fk_on_update.rs docs.
                let fk_update_plan = super::fk_on_update::plan_fk_on_update(
                    self.resolver,
                    table_ref,
                    &table,
                    op_ref,
                    &ctx,
                    alias,
                )
                .await?;

                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => {
                        // Apply child fan-out (cascade/setnull) inside the
                        // explicit tx, BEFORE the parent update. Child rows
                        // reference parent *values* (not row identity), so
                        // re-keying them first keeps the tx consistent.
                        super::fk_on_update::apply_fk_update_plan(fk_update_plan, tx, alias)
                            .await?;
                        table
                            .execute_update_tx(op_ref, &ctx, tx, Some(self.resolver))
                            .await
                            .map_err(|e| BatchError::QueryError {
                                alias: alias.to_string(),
                                message: e.to_string(),
                                code: None,
                            })?
                    }
                    // F4b-2: "everything is a transaction" — a non-tx update
                    // routes through the implicit single-op BATCH transaction
                    // (same pattern as INSERT in F4b-1).
                    None => {
                        let repo =
                            self.resolver
                                .resolve_repo(&table_ref.repo)
                                .await
                                .map_err(|e| BatchError::QueryError {
                                    alias: alias.to_string(),
                                    message: format!("resolve_repo({}): {}", table_ref.repo, e),
                                    code: None,
                                })?;
                        let owned_op: shamir_query_types::write::UpdateOp = op_ref.clone();
                        let owned_table = table.clone();
                        let owned_refs = resolved_refs.clone();
                        let owned_params = self.params.clone();
                        let owned_actor = self.actor.clone();
                        let owned_alias = alias.to_string();
                        repo.run_implicit_batch_tx(self.actor.clone(), alias, move |tx| {
                            Box::pin(async move {
                                // Apply child fan-out inside the implicit tx
                                // BEFORE the parent update. The plan carries
                                // pre-resolved child handles (no resolver
                                // needed inside the closure).
                                super::fk_on_update::apply_fk_update_plan(
                                    fk_update_plan,
                                    tx,
                                    &owned_alias,
                                )
                                .await
                                .map_err(|e| {
                                    shamir_storage::error::DbError::Validation(e.to_string())
                                })?;

                                let interner = owned_table.interner().get().await?;
                                let ctx = FilterContext::new(interner, &owned_refs)
                                    .with_actor(owned_actor)
                                    .with_params(&owned_params);
                                owned_table
                                    .execute_update_tx(&owned_op, &ctx, tx, None)
                                    .await
                            })
                        })
                        .await?
                    }
                };
                Ok(write_result_to_query_result_with_encoding(
                    wr,
                    self.result_encoding,
                    interner,
                ))
            }

            BatchOp::Delete(op) => {
                // Observability trace only — see `trace_access`'s doc
                // comment. Real enforcement already ran in execute_as /
                // tx_execute_as before this runner was reached.
                trace_access(&self.actor, &resource, Action::Delete).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;

                // Phase D.1 — ON DELETE RESTRICT gate.
                //
                // TOCTOU caveat: this check runs BEFORE the delete, outside the
                // atomic scope of the tx that performs the removal. A concurrent
                // insert into a child table between this check and the delete
                // could create a dangling reference. Tightening to an in-tx
                // atomic check requires Arc<dyn TableResolver> (future refactor).
                // Acceptable for MVP: delete is not a hot path, Restrict tables
                // are opt-in, and the window is small.
                super::fk_restrict::check_fk_restrict(
                    self.resolver,
                    table_ref,
                    &table,
                    &op.where_clause,
                    &ctx,
                    alias,
                )
                .await?;

                // Phase D.2 — ON DELETE CASCADE + SET NULL.
                //
                // Build the cascade/setnull plan BEFORE the tx closure (the
                // resolver cannot be captured into the HRTB closure — same
                // constraint as the RESTRICT gate). The plan is owned data
                // (pre-resolved child TableManager handles + parent values);
                // the full cascade tree (including grandchildren) is resolved
                // at planning time so no resolver is needed inside the tx.
                let cascade_plan = super::fk_actions::plan_cascade(
                    self.resolver,
                    table_ref,
                    &table,
                    &op.where_clause,
                    &ctx,
                    alias,
                )
                .await?;

                let wr = match self.tx.as_deref_mut() {
                    Some(tx) => {
                        // Apply cascade/setnull inside the explicit tx BEFORE
                        // the parent delete (child rows reference parent values,
                        // not parent row existence at mutation time; ordering
                        // is cleaner this way).
                        super::fk_actions::apply_cascade_plan(cascade_plan, tx, alias).await?;
                        table
                            .execute_delete_tx(op, &ctx, tx, Some(self.resolver))
                            .await
                            .map_err(|e| BatchError::QueryError {
                                alias: alias.to_string(),
                                message: e.to_string(),
                                code: None,
                            })?
                    }
                    // F4b-3: "everything is a transaction" — a non-tx delete
                    // routes through the implicit single-op BATCH transaction
                    // (same pattern as INSERT in F4b-1 and UPDATE in F4b-2).
                    None => {
                        let repo =
                            self.resolver
                                .resolve_repo(&table_ref.repo)
                                .await
                                .map_err(|e| BatchError::QueryError {
                                    alias: alias.to_string(),
                                    message: format!("resolve_repo({}): {}", table_ref.repo, e),
                                    code: None,
                                })?;
                        let owned_op: shamir_query_types::write::DeleteOp = op.clone();
                        let owned_table = table.clone();
                        let owned_refs = resolved_refs.clone();
                        let owned_params = self.params.clone();
                        let owned_actor = self.actor.clone();
                        let owned_alias = alias.to_string();
                        repo.run_implicit_batch_tx(self.actor.clone(), alias, move |tx| {
                            Box::pin(async move {
                                // Apply cascade/setnull inside the implicit tx
                                // BEFORE the parent delete. The plan carries
                                // pre-resolved child handles (no resolver needed
                                // inside the closure).
                                super::fk_actions::apply_cascade_plan(
                                    cascade_plan,
                                    tx,
                                    &owned_alias,
                                )
                                .await
                                .map_err(|e| {
                                    shamir_storage::error::DbError::Validation(e.to_string())
                                })?;

                                let interner = owned_table.interner().get().await?;
                                let ctx = FilterContext::new(interner, &owned_refs)
                                    .with_actor(owned_actor)
                                    .with_params(&owned_params);
                                owned_table
                                    .execute_delete_tx(&owned_op, &ctx, tx, None)
                                    .await
                            })
                        })
                        .await?
                    }
                };
                Ok(write_result_to_query_result_with_encoding(
                    wr,
                    self.result_encoding,
                    interner,
                ))
            }

            BatchOp::Set(op) => {
                // Observability trace only — see `trace_access`'s doc
                // comment. Real enforcement already ran in execute_as /
                // tx_execute_as before this runner was reached.
                trace_access(&self.actor, &resource, Action::Write).map_err(|e| {
                    BatchError::QueryError {
                        alias: alias.to_string(),
                        message: e.to_string(),
                        code: None,
                    }
                })?;
                // #641: resolve reserved markers ($param/$query/$fn/$cond/
                // $expr) inside the upsert `key` and `value`.
                // `resolve_write_value` has a fast path (clone) when no
                // marker nodes exist.
                let subst_key = resolve_write_value(&op.key, &ctx)
                    .map_err(|e| write_value_error_to_batch_error(alias, e))?;
                let subst_value = resolve_write_value(&op.value, &ctx)
                    .map_err(|e| write_value_error_to_batch_error(alias, e))?;
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
                    Some(tx) => table
                        .execute_set_tx(op_ref, tx, Some(self.resolver))
                        .await
                        .map_err(|e| BatchError::QueryError {
                            alias: alias.to_string(),
                            message: e.to_string(),
                            code: None,
                        })?,
                    // W3d-2: non-tx SET routes through the implicit single-op
                    // batch transaction (same pattern as DELETE in F5a, INSERT
                    // in F4b-1, UPDATE in F4b-2).
                    None => {
                        let repo =
                            self.resolver
                                .resolve_repo(&table_ref.repo)
                                .await
                                .map_err(|e| BatchError::QueryError {
                                    alias: alias.to_string(),
                                    message: format!("resolve_repo({}): {}", table_ref.repo, e),
                                    code: None,
                                })?;
                        let owned_op: shamir_query_types::write::SetOp = op_ref.clone();
                        let owned_table = table.clone();
                        let owned_actor = self.actor.clone();
                        repo.run_implicit_batch_tx(owned_actor, alias, move |tx| {
                            Box::pin(async move {
                                owned_table.execute_set_tx(&owned_op, tx, None).await
                            })
                        })
                        .await?
                    }
                };
                Ok(write_result_to_query_result_with_encoding(
                    wr,
                    self.result_encoding,
                    interner,
                ))
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
    params: &TMap<String, QueryValue>,
    result_encoding: ResultEncoding,
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
    };
    runner.run(alias, entry, resolved_refs).await
}

/// Convert WriteResult to QueryResult for BatchResponse compatibility.
///
/// C1: fold each already-built [`InsertedRecord`] straight into a
/// [`QueryRecord::Inserted`] — no re-materialisation.
/// The old path serialised every row a SECOND time (per-record map
/// allocation) just to wrap it in `QueryRecord::Inserted`;
/// `QueryRecord::Inserted` serialises via the same `InsertedRecord` impl, so
/// the wire bytes are byte-identical while the duplicate build is gone.
///
/// When `encoding == ResultEncoding::Id`, each RETURNING row is re-encoded
/// into [`QueryRecord::IdBytes`] (id-keyed storage msgpack) via the table's
/// interner — matching the Id-encoded read path.  Keys that fail to intern
/// (unknown field names) are silently skipped; if encoding fails for a row
/// it falls back to `QueryRecord::Inserted` so no data is lost.
pub(super) fn write_result_to_query_result_with_encoding(
    wr: WriteResult,
    encoding: ResultEncoding,
    interner: &Interner,
) -> QueryResult {
    let records: Vec<QueryRecord> =
        if matches!(encoding, ResultEncoding::Id) && !wr.records.is_empty() {
            let mut scratch = Vec::new();
            wr.records
                .into_iter()
                .map(|rec| {
                    // Re-encode the name-keyed fields to id-keyed storage bytes.
                    // `get_ind` returns None for unknown keys — those are silently
                    // omitted, consistent with the read-path projection behaviour.
                    let intern_fn = |key: &str| {
                        interner.get_ind(key).ok_or_else(|| {
                            shamir_types::codecs::CodecError::Decode(format!(
                                "write_result_to_query_result_with_encoding: unknown field '{key}'"
                            ))
                        })
                    };
                    match query_value_to_storage_bytes_into(&rec.fields, &intern_fn, &mut scratch) {
                        Ok(bytes) => QueryRecord::IdBytes(ByteBuf::from(bytes.as_ref())),
                        // Encoding failed (e.g. non-map value) — fall back gracefully.
                        Err(_) => QueryRecord::from(rec),
                    }
                })
                .collect()
        } else {
            wr.records.into_iter().map(QueryRecord::from).collect()
        };
    QueryResult {
        records,
        stats: Some(QueryStats {
            index_used: None,
            records_scanned: wr.affected,
            records_returned: wr.affected,
            execution_time_us: wr.execution_time_us,
        }),
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
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
        // Value-vs-value comparison has no field/record involvement — not
        // a per-row predicate the live subscription bridge evaluator can
        // apply against a row's field data.
        Filter::ValueCompare { .. } => Some("value_compare"),
    }
}

// Re-export public items used outside this module
pub use crate::query::batch::batch_execute::execute_batch;
#[cfg(test)]
pub use crate::query::batch::batch_execute::execute_batch_with_permissions;
pub use crate::query::batch::interactive_tx::{
    commit_interactive_tx, execute_in_open_tx, open_interactive_tx,
};
