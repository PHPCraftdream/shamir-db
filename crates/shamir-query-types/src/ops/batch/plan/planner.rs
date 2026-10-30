//! Batch query planner.
//!
//! Analyzes dependencies between queries and creates an execution plan
//! that maximizes parallelism while respecting dependencies.
//!
//! # How It Works
//!
//! 1. **Extract Dependencies**: Scans all filters for `$query` references
//! 2. **Validate**: Checks for unknown aliases and cycles
//! 3. **Calculate Depth**: Ensures dependency chain isn't too deep
//! 4. **Topological Sort**: Groups queries into LOGICAL stages
//!
//! # Stages are a logical grouping, not a parallelism guarantee
//!
//! A "stage" groups queries whose dependencies are all satisfied by earlier
//! stages — i.e. queries within one stage are independent of each other and
//! COULD run concurrently. Whether the executor actually drives them
//! concurrently is a separate, independent decision: the current executor
//! (`shamir-engine::query::batch::batch_execute::execute_plan_impl`) runs
//! every stage's queries sequentially on one task. A `try_join_all`-based
//! concurrent-stage experiment was tried and measured as a no-op for
//! in-memory CPU-bound workloads (no `.await` suspension points to yield
//! on); see that module's doc comment for the measurement and the
//! `tokio::spawn`-per-query design this is deferred to. See
//! `docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md` for the
//! decision record.
//!
//! # $query Validation Strategy
//!
//! We use **strict validation**: all `$query` references must point to aliases
//! that exist in the same batch. If a reference is invalid, we return
//! `BatchError::UnknownAlias` at planning time.
//!
//! **Why strict validation?**
//! - Catches typos immediately (e.g., `"usres"` instead of `"users"`)
//! - Prevents silent data corruption in write operations
//! - Fails fast with clear error instead of producing wrong results
//! - Dependencies are auto-extracted, so there's no way to "lie" about them
//!
//! See `batch/README.md` for detailed rationale.
//!
//! # Example
//!
//! ```text
//! Input queries: { users: {...}, products: {...}, orders: {...}, stats: {...} }
//!
//! Dependencies:
//!   users -> {}
//!   products -> {}
//!   orders -> {users, products}
//!   stats -> {orders}
//!
//! Output stages: [[users, products], [orders], [stats]]
//! ```
//!
//! Stage 1 runs `users` and `products` in parallel.
//! Stage 2 runs `orders` after Stage 1 completes.
//! Stage 3 runs `stats` after Stage 2 completes.

use std::collections::VecDeque;

use crate::batch::alias::split_path_tail;
use crate::batch::{
    BatchError, BatchLimits, BatchOp, BatchPlan, BatchTreeVisitor, EdgeKind, QueryEntry,
};
use crate::filter::{Filter, FilterValue};
use crate::read::SelectExpr;
use shamir_collections::{new_map, new_set, TFxSet, TMap, TSet};
use shamir_types::types::value::QueryValue;

/// Maximum nesting depth for the planner's value/filter extraction walks.
/// Bounds native recursion on attacker-shaped trees BEFORE the engine's own
/// filter-depth validation runs (planning precedes it).
pub(super) const MAX_EXTRACT_DEPTH: usize = 256;

/// Batch query planner.
///
/// Creates execution plans from query entries with automatic
/// dependency detection and parallel stage grouping.
///
/// # Example
///
/// ```rust,ignore
/// use shamir_db::query::batch::{BatchPlanner, BatchLimits, QueryEntry};
/// use shamir_db::types::common::new_map;
///
/// let mut queries = new_map();
/// queries.insert("users".to_string(), QueryEntry::from(ReadQuery::new("users")));
/// queries.insert("orders".to_string(), QueryEntry::from(ReadQuery::new("orders")));
///
/// let plan = BatchPlanner::plan(&queries, &BatchLimits::default())?;
/// println!("Stages: {:?}", plan.stages);
/// ```
pub struct BatchPlanner;

impl BatchPlanner {
    /// Create an execution plan from query entries.
    ///
    /// # Returns
    ///
    /// - `Ok(BatchPlan)` with stages for parallel execution
    /// - `Err(BatchError)` if validation fails
    ///
    /// # Errors
    ///
    /// - `TooManyQueries`: More queries than `limits.max_queries`
    /// - `UnknownAlias`: Reference to non-existent alias
    /// - `CircularDependency`: Cycle in dependency graph
    /// - `TooDeep`: Dependency chain exceeds `limits.max_dependency_depth`
    pub fn plan(
        queries: &TMap<String, QueryEntry>,
        limits: &BatchLimits,
    ) -> Result<BatchPlan, BatchError> {
        // Check query count
        if queries.len() > limits.max_queries {
            return Err(BatchError::TooManyQueries {
                count: queries.len(),
                max: limits.max_queries,
            });
        }

        // Check static sub-batch nesting depth using the shared iterative visitor.
        let nesting = Self::max_nesting_depth_of_ops(queries);
        if nesting > limits.max_nesting_depth {
            return Err(BatchError::NestingTooDeep {
                depth: nesting,
                max: limits.max_nesting_depth,
            });
        }

        // Static DoS gate for `ForEach` nodes whose `over` is a literal
        // array (Epic04/B, #653; ADR Decision 3): the iteration count is
        // known at plan time, so fold `iterations × body.len()` into the
        // SAME budget `max_queries` already enforces — a `ForEach` node
        // contributes that product as "virtual op units" to the parent's
        // total, not just its own single node. This closes the DoS hole a
        // small-looking `ForEach` wrapping a large body would otherwise
        // open. Dynamic `over` (a `$query`-column-ref) has no known length
        // at plan time — the planner is a pure DTO crate with no I/O — so
        // it is gated only by the runtime check immediately before
        // iteration 0 (`BatchLimits::max_iterations`, engine-side).
        // Units are summed across all sibling nodes before the single check,
        // so two literal `ForEach` products accumulate into the same budget.
        let mut static_units = 0usize;
        for entry in queries.values() {
            let units = match &entry.op {
                BatchOp::ForEach(fe) => match &fe.over {
                    crate::filter::FilterValue::Array(items) => {
                        items.len().saturating_mul(fe.batch.queries.len()).max(1)
                    }
                    _ => 1,
                },
                _ => 1,
            };
            static_units = static_units.saturating_add(units);
        }
        if static_units > limits.max_queries {
            return Err(BatchError::TooManyQueries {
                count: static_units,
                max: limits.max_queries,
            });
        }

        let mut total_queries = 0u64;
        let mut total_work = 0u64;
        for visit in BatchTreeVisitor::new(queries) {
            total_queries = total_queries.saturating_add(1);
            total_work = total_work.saturating_add(visit.multiplier);
        }
        if total_queries > limits.max_total_queries as u64 {
            return Err(BatchError::LimitExceeded {
                kind: crate::batch::LimitKind::TotalQueries,
                actual: total_queries,
                max: limits.max_total_queries as u64,
            });
        }
        if total_work > limits.max_total_work {
            return Err(BatchError::LimitExceeded {
                kind: crate::batch::LimitKind::Work,
                actual: total_work,
                max: limits.max_total_work,
            });
        }

        // Aliases are keys in the map (no duplicates possible)
        let alias_order: Vec<&str> = queries.keys().map(String::as_str).collect();

        // Extract dependencies for each query, tagging each edge with its
        // provenance (Explicit from `after`, DataFlow from `$query`, or
        // Both when the same alias is named by both sources).
        let mut dependencies: TMap<&str, TSet<&str>> = new_map();
        let mut edge_provenance: TMap<&str, TMap<&str, EdgeKind>> = new_map();

        for (alias, entry) in queries {
            let mut data_flow_deps = Self::extract_dependencies(&entry.op, alias)?;

            // Epic03/B (#645): `when: Option<Filter>` participates in the
            // DAG exactly like a WHERE clause's `$query` refs — a `when`
            // that gates on another alias's result must run after that
            // alias, and (per ADR Decision 2) must cascade-skip if that
            // alias is itself skipped. Reuses the same (bug-#642-fixed)
            // `extract_deps_from_filter`/`extract_deps_from_filter_value`
            // leaf extractor as WHERE.
            if let Some(when_filter) = &entry.when {
                // #651 defensive check: reject OLD field-based comparison
                // variants inside `when` BEFORE any execution — they
                // resolve a `FieldPath` against a record that does not
                // exist in a `when` context, and would otherwise silently
                // fold to a fixed result (see `QueryRunner::resolve_skip`).
                if Self::contains_field_based_comparison(when_filter, 0) {
                    return Err(BatchError::InvalidWhenFilter {
                        alias: alias.clone(),
                        message: "field-based comparisons are not meaningful inside `when` \
                            (no record exists) — use Filter::ValueCompare for value-vs-value \
                            comparisons instead"
                            .to_string(),
                    });
                }
                Self::extract_deps_from_filter(when_filter, &mut data_flow_deps, 0)?;
            }

            let mut provenance: TMap<String, EdgeKind> = data_flow_deps
                .into_iter()
                .map(|dep| (dep, EdgeKind::DataFlow))
                .collect();

            // Merge explicit ordering dependencies from `after`. A garbage
            // path tail (e.g. "mk[0].id") is rejected: `after` is alias-only
            // ordering and never resolves a value path the way `$query`
            // does, so a path tail here is almost always the developer
            // mistakenly expecting `after` to behave like `$query`. Failing
            // fast at plan time is more useful than silently stripping to
            // the base alias.
            for raw in &entry.after {
                if let Some((base, path)) = split_path_tail(raw) {
                    return Err(BatchError::AfterPathIgnored {
                        alias: alias.clone(),
                        raw: format!("{}{}", base, path),
                    });
                }
                let base = Self::extract_base_alias(raw);
                provenance
                    .entry(base)
                    .and_modify(|kind| *kind = kind.merge(EdgeKind::Explicit))
                    .or_insert(EdgeKind::Explicit);
            }

            // Validate all referenced aliases exist
            for dep in provenance.keys() {
                if !queries.contains_key(dep) {
                    return Err(BatchError::UnknownAlias {
                        alias: dep.clone(),
                        referenced_by: alias.clone(),
                    });
                }
            }

            let borrowed_provenance: TMap<&str, EdgeKind> = provenance
                .iter()
                .map(|(dep, kind)| {
                    (
                        queries
                            .get_key_value(dep)
                            .expect("validated alias")
                            .0
                            .as_str(),
                        *kind,
                    )
                })
                .collect();
            dependencies.insert(
                alias.as_str(),
                borrowed_provenance.keys().copied().collect(),
            );
            edge_provenance.insert(alias.as_str(), borrowed_provenance);
        }

        // Check for cycles
        if let Some(cycle) = Self::detect_cycle(&dependencies) {
            return Err(BatchError::CircularDependency { cycle });
        }

        // Check depth
        let depth = Self::calculate_max_depth(&dependencies);
        if depth > limits.max_dependency_depth {
            return Err(BatchError::TooDeep {
                depth,
                max: limits.max_dependency_depth,
            });
        }

        // Topological sort into stages
        let stages = Self::topological_sort(&dependencies, &alias_order);

        Ok(BatchPlan {
            stages,
            aliases: alias_order.into_iter().map(str::to_owned).collect(),
            dependencies: dependencies
                .into_iter()
                .map(|(alias, deps)| {
                    (
                        alias.to_owned(),
                        deps.into_iter().map(str::to_owned).collect(),
                    )
                })
                .collect(),
            edge_provenance: edge_provenance
                .into_iter()
                .map(|(alias, edges)| {
                    (
                        alias.to_owned(),
                        edges
                            .into_iter()
                            .map(|(dep, kind)| (dep.to_owned(), kind))
                            .collect(),
                    )
                })
                .collect(),
        })
    }

    /// Extract all query references from a batch operation.
    ///
    /// `alias` is the batch alias `op` belongs to — threaded through purely
    /// for error attribution: #663's `InvalidCondCondition` (raised deep
    /// inside [`Self::extract_deps_from_value`] when a write-value `$cond`'s
    /// condition is field-based) needs to name the offending alias.
    fn extract_dependencies(op: &BatchOp, alias: &str) -> Result<TSet<String>, BatchError> {
        let mut deps = new_set();

        match op {
            BatchOp::Read(query) => {
                if let Some(filter) = &query.r#where {
                    Self::extract_deps_from_filter(filter, &mut deps, 0)?;
                }
                for item in &query.select.items {
                    match item {
                        crate::read::SelectItem::Function { args, .. } => {
                            for arg in args {
                                Self::extract_deps_from_filter_value(arg, &mut deps, 0)?;
                            }
                        }
                        crate::read::SelectItem::AggregateFn { args, input, .. } => {
                            for arg in args {
                                Self::extract_deps_from_filter_value(arg, &mut deps, 0)?;
                            }
                            if let Some(input) = input {
                                Self::extract_deps_from_filter_value(input, &mut deps, 0)?;
                            }
                        }
                        crate::read::SelectItem::Aggregate { input, .. } => {
                            if let Some(input) = input {
                                Self::extract_deps_from_filter_value(input, &mut deps, 0)?;
                            }
                        }
                        crate::read::SelectItem::Expression { expr, .. } => {
                            // `to_filter_value` recurses with no depth
                            // state: probe the INPUT tree iteratively
                            // (mirrors `value_height_bounded`) first.
                            if !Self::select_expr_height_bounded(expr, MAX_EXTRACT_DEPTH) {
                                return Err(BatchError::NestingTooDeep {
                                    depth: MAX_EXTRACT_DEPTH,
                                    max: MAX_EXTRACT_DEPTH,
                                });
                            }
                            let fv = expr.to_filter_value();
                            Self::extract_deps_from_filter_value(&fv, &mut deps, 0)?;
                        }
                        crate::read::SelectItem::All
                        | crate::read::SelectItem::Field { .. }
                        | crate::read::SelectItem::CountAll { .. } => {}
                    }
                }
                // RV-QUERY-BUILDER-E8D88CD7: a HAVING `$query` ref is a data
                // ref like WHERE's; without this edge `build_resolved_refs`
                // never exposes the producer's result to HAVING.
                if let Some(having) = query.group_by.as_ref().and_then(|g| g.having.as_ref()) {
                    Self::extract_deps_from_filter(having, &mut deps, 0)?;
                }
            }
            BatchOp::Update(update) => {
                if let Some(filter) = &update.where_clause {
                    Self::extract_deps_from_filter(filter, &mut deps, 0)?;
                }
                Self::extract_deps_from_value(&update.set, &mut deps, alias, 0)?;
            }
            BatchOp::Set(set) => {
                Self::extract_deps_from_value(&set.key, &mut deps, alias, 0)?;
                Self::extract_deps_from_value(&set.value, &mut deps, alias, 0)?;
            }
            BatchOp::Delete(delete) => {
                Self::extract_deps_from_filter(&delete.where_clause, &mut deps, 0)?;
            }
            BatchOp::Insert(insert) => {
                for value in &insert.values {
                    Self::extract_deps_from_value(value, &mut deps, alias, 0)?;
                }
            }
            // Stored-procedure call: scan positional params for `$query` refs
            // so a Call participates in the topological order (runs after any
            // query whose result it consumes as an argument).
            BatchOp::Call(call_op) => {
                for fv in &call_op.params {
                    Self::extract_deps_from_filter_value(fv, &mut deps, 0)?;
                }
            }
            // Sub-batch: outer deps come exclusively from `bind` values.
            // Do NOT descend into the inner batch's queries — those are
            // planned recursively at execution time (P3).
            BatchOp::Batch(sub) => {
                for fv in sub.bind.values() {
                    Self::extract_deps_from_filter_value(fv, &mut deps, 0)?;
                }
            }
            // ForEach loop (Epic04/B, #653): outer deps come exclusively
            // from `over` — the iteration source. Do NOT descend into the
            // loop body's queries — those are planned recursively at
            // execution time (K times), mirroring `Batch(sub)` above.
            BatchOp::ForEach(fe) => {
                Self::extract_deps_from_filter_value(&fe.over, &mut deps, 0)?;
            }
            // Admin ops have no query dependencies
            _ => {}
        }

        Ok(deps)
    }

    /// Extract dependencies from a write-row `QueryValue`
    /// (`InsertOp.values[i]`/`UpdateOp.set`/`SetOp.{key,value}`).
    ///
    /// #641 fix: `$query` detection used to be LOOSE (`map.get("$query")`
    /// anywhere, no recursion into `$fn`/`$cond`/`$expr` operands), so a
    /// `$query` ref nested inside one of those silently dropped out of the
    /// dependency graph (wrong topological order). This now matches the
    /// precision AND recursion depth of
    /// [`Self::extract_deps_from_filter_value`] (the #642 fix): a marker is
    /// detected with the exact reserved-key-map convention (mirroring
    /// `contains_param_ref`'s `is_marker_map` in `shamir-engine`'s
    /// `param_subst.rs`), then decoded into a
    /// [`crate::filter::FilterValue`] using the shared buffered-value decoder,
    ///
    /// `$param` is intentionally NOT treated as a marker here: it carries no
    /// alias to depend on (it reads from the sub-batch's `bind` scope, not
    /// another query's result), so it contributes no dependency edge either
    /// way — same as the old code's silent no-op for it.
    ///
    /// #663 Gap 3 fix: when the decoded marker is a `FilterValue::Cond`, its
    /// `condition` is validated with the SAME `contains_field_based_
    /// comparison` guard `when` already uses — `resolve_write_value`
    /// (`shamir-engine`'s `param_subst.rs`) evaluates a write-value `$cond`'s
    /// condition against a record-less dummy, exactly like `when`'s
    /// `resolve_skip`, so a field-based comparison there is equally
    /// meaningless and would otherwise silently fold instead of erroring.
    /// `alias` is threaded through purely for error attribution on that new
    /// `BatchError::InvalidCondCondition`.
    fn extract_deps_from_value(
        value: &QueryValue,
        deps: &mut TSet<String>,
        alias: &str,
        depth: usize,
    ) -> Result<(), BatchError> {
        if depth > MAX_EXTRACT_DEPTH {
            return Err(BatchError::NestingTooDeep {
                depth,
                max: MAX_EXTRACT_DEPTH,
            });
        }
        match value {
            QueryValue::Map(map) => {
                // Reserved-key marker detection (mirrors `is_marker_map` in
                // `shamir-engine`'s `param_subst.rs`) — `$param` excluded
                // (see doc comment above). `$query`'s serde shape is
                // `{"$query": "<alias>", "path"?: "<path>"}` — a SECOND
                // top-level `path` key is the normal, common case (any
                // `$query` ref that carries a path), so a 2-key map with
                // exactly `$query`+`path` is ALSO a marker, not just the
                // 1-key case `$fn`/`$cond`/`$expr` always are.
                let is_query_fn_cond_expr_marker = match map.len() {
                    1 => ["$query", "$fn", "$cond", "$expr"]
                        .iter()
                        .any(|k| map.contains_key(*k)),
                    2 => map.contains_key("$query") && map.contains_key("path"),
                    _ => false,
                };
                if is_query_fn_cond_expr_marker
                    && Self::value_height_bounded(value, MAX_EXTRACT_DEPTH - depth)
                {
                    if let Ok(fv) = FilterValue::try_from_query_value_ref(value) {
                        if let FilterValue::Cond { cond } = &fv {
                            // Check the top-level condition, then recurse
                            // into `then`/`or_else` in case the guard-worthy
                            // `$cond` is nested deeper inside a branch value
                            // (a `$cond`'s branches can themselves be
                            // `$cond`s).
                            let invalid =
                                Self::contains_field_based_comparison(&cond.condition, depth + 1)
                                    || Self::filter_value_contains_field_based_comparison(
                                        &cond.then,
                                        depth + 1,
                                    )
                                    || Self::filter_value_contains_field_based_comparison(
                                        &cond.or_else,
                                        depth + 1,
                                    );
                            if invalid {
                                return Err(BatchError::InvalidCondCondition {
                                    alias: alias.to_string(),
                                    message: "field-based comparisons are not meaningful \
                                        inside `$cond`'s condition when used in a write value \
                                        (no record exists) — use Filter::ValueCompare for \
                                        value-vs-value comparisons instead"
                                        .to_string(),
                                });
                            }
                        }
                        Self::extract_deps_from_filter_value(&fv, deps, depth + 1)?;
                        return Ok(());
                    }
                    // Malformed marker payload: fall through to plain
                    // recursion below — execution-time resolution
                    // (`param_subst.rs`) is the authority that rejects
                    // malformed markers with a clear error; the planner's
                    // dependency pass stays best-effort.
                }
                // Recurse into nested objects (also covers `$param` markers
                // and non-marker maps).
                for v in map.values() {
                    Self::extract_deps_from_value(v, deps, alias, depth + 1)?;
                }
            }
            QueryValue::List(arr) => {
                for v in arr {
                    Self::extract_deps_from_value(v, deps, alias, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Extract dependencies from a filter. Depth-bounded: `depth` caps the
    /// recursion so an attacker-shaped filter tree cannot overflow the stack
    /// before the engine's own filter-depth validation runs.
    fn extract_deps_from_filter(
        filter: &Filter,
        deps: &mut TSet<String>,
        depth: usize,
    ) -> Result<(), BatchError> {
        if depth > MAX_EXTRACT_DEPTH {
            return Err(BatchError::NestingTooDeep {
                depth,
                max: MAX_EXTRACT_DEPTH,
            });
        }
        match filter {
            Filter::Eq { value, .. }
            | Filter::Ne { value, .. }
            | Filter::Gt { value, .. }
            | Filter::Gte { value, .. }
            | Filter::Lt { value, .. }
            | Filter::Lte { value, .. }
            | Filter::Contains { value, .. } => {
                Self::extract_deps_from_filter_value(value, deps, depth + 1)?;
            }
            Filter::In { values, .. } | Filter::NotIn { values, .. } => {
                for v in values {
                    Self::extract_deps_from_filter_value(v, deps, depth + 1)?;
                }
            }
            Filter::Between { from, to, .. } => {
                Self::extract_deps_from_filter_value(from, deps, depth + 1)?;
                Self::extract_deps_from_filter_value(to, deps, depth + 1)?;
            }
            Filter::ContainsAny { values, .. } | Filter::ContainsAll { values, .. } => {
                for v in values {
                    Self::extract_deps_from_filter_value(v, deps, depth + 1)?;
                }
            }
            Filter::And { filters } | Filter::Or { filters } => {
                for f in filters {
                    Self::extract_deps_from_filter(f, deps, depth + 1)?;
                }
            }
            Filter::Not { filter } => {
                Self::extract_deps_from_filter(filter, deps, depth + 1)?;
            }
            Filter::FieldEq { value, .. } => {
                Self::extract_deps_from_filter_value(value, deps, depth + 1)?;
            }
            // Filters without FilterValue
            Filter::Like { .. }
            | Filter::ILike { .. }
            | Filter::Regex { .. }
            | Filter::IsNull { .. }
            | Filter::IsNotNull { .. }
            | Filter::Exists { .. }
            | Filter::NotExists { .. }
            | Filter::Fts { .. }
            | Filter::VectorSimilarity { .. } => {}
            Filter::Computed {
                value, expr_args, ..
            } => {
                Self::extract_deps_from_filter_value(value, deps, depth + 1)?;
                if let Some(args) = expr_args {
                    for v in args {
                        Self::extract_deps_from_filter_value(v, deps, depth + 1)?;
                    }
                }
            }
            Filter::ValueCompare { left, right, .. } => {
                Self::extract_deps_from_filter_value(left, deps, depth + 1)?;
                Self::extract_deps_from_filter_value(right, deps, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Extract dependencies from a filter value.
    ///
    /// Bug #642 fix: `FilterValue::Cond`/`Expr`/`FnCall` can each carry a
    /// nested `FilterValue::QueryRef` (a `$query` reference inside a
    /// `$cond`'s `then`/`or_else`, an `$expr`'s `args`, or a `FnCall`'s
    /// `args`) — these must recurse into the same extractor, otherwise the
    /// referenced alias silently drops out of the dependency graph (wrong
    /// topological order, and — for `when` — no cascade on skip). This is
    /// the single shared leaf-level extractor for both WHERE-`Filter` trees
    /// and (Epic03/B) `when`-`Filter` trees; fixing it here benefits both.
    /// Depth-bounded via `depth`/[`MAX_EXTRACT_DEPTH`] — the planner runs
    /// before the engine's filter-depth validation.
    fn extract_deps_from_filter_value(
        value: &crate::filter::FilterValue,
        deps: &mut TSet<String>,
        depth: usize,
    ) -> Result<(), BatchError> {
        if depth > MAX_EXTRACT_DEPTH {
            return Err(BatchError::NestingTooDeep {
                depth,
                max: MAX_EXTRACT_DEPTH,
            });
        }
        match value {
            FilterValue::Array(arr) => {
                for v in arr {
                    Self::extract_deps_from_filter_value(v, deps, depth + 1)?;
                }
            }
            FilterValue::QueryRef { alias, .. } => {
                let base_alias = Self::extract_base_alias(alias);
                deps.insert(base_alias);
            }
            FilterValue::Cond { cond } => {
                Self::extract_deps_from_filter(&cond.condition, deps, depth + 1)?;
                Self::extract_deps_from_filter_value(&cond.then, deps, depth + 1)?;
                Self::extract_deps_from_filter_value(&cond.or_else, deps, depth + 1)?;
            }
            FilterValue::Expr { expr } => {
                for arg in &expr.args {
                    Self::extract_deps_from_filter_value(arg, deps, depth + 1)?;
                }
            }
            FilterValue::FnCall { call } => {
                for arg in call.args() {
                    Self::extract_deps_from_filter_value(arg, deps, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Extract base alias from a `$query` reference string like
    /// `"@users[0].id"` → `"users"`.
    ///
    /// Thin delegate to [`crate::batch::alias::extract_base_alias`], which
    /// is the shared, crate-public normalization (Epic01/B reuses it from
    /// `shamir-query-builder`). Kept as an inherent method here so the
    /// existing `Self::extract_base_alias(..)` call sites are unchanged.
    fn extract_base_alias(s: &str) -> String {
        crate::batch::alias::extract_base_alias(s)
    }

    /// Detect cycle in the dependency graph — ITERATIVE DFS (white-gray-black
    /// with an explicit stack). The cycle check precedes the depth rejection,
    /// so its walk depth must not scale with an attacker-controlled alias
    /// chain length.
    fn detect_cycle(deps: &TMap<&str, TSet<&str>>) -> Option<Vec<String>> {
        // Borrow-based sets — no allocations in the happy path.
        let mut white: TFxSet<&str> = deps.keys().copied().collect();
        let mut black: TFxSet<&str> = TFxSet::default();

        // Collect keys once so we can iterate without borrowing `white` mutably.
        let all_nodes: Vec<&str> = deps.keys().copied().collect();
        for start in all_nodes {
            if !white.contains(start) {
                continue;
            }
            // Explicit-stack DFS: gray set == nodes on the current path.
            let mut gray: TFxSet<&str> = TFxSet::default();
            let mut path: Vec<&str> = Vec::new();
            // (node, next neighbor index)
            let mut stack: Vec<(&str, usize)> = Vec::new();
            white.remove(start);
            gray.insert(start);
            path.push(start);
            stack.push((start, 0));

            while let Some(top) = stack.last_mut() {
                let (node, idx) = *top;
                let next = deps.get(node).and_then(|ns| ns.iter().nth(idx));
                if let Some(n) = next {
                    top.1 += 1;
                    let nb: &str = n;
                    if gray.contains(nb) {
                        // Found cycle — allocate only here (error path).
                        let cycle_start = path.iter().position(|&x| x == nb)?;
                        return Some(path[cycle_start..].iter().map(|&s| s.to_string()).collect());
                    }
                    if white.contains(nb) {
                        white.remove(nb);
                        gray.insert(nb);
                        path.push(nb);
                        stack.push((nb, 0));
                    }
                } else {
                    stack.pop();
                    gray.remove(node);
                    black.insert(node);
                    path.pop();
                }
            }
        }

        None
    }

    /// Iterative (explicit-stack) check that the `QueryValue` tree's height
    /// is at most `cap`. Used to prove that a recursive serde serialization
    /// of `value` cannot overflow the native stack before it is attempted.
    fn value_height_bounded(value: &QueryValue, cap: usize) -> bool {
        let mut stack: Vec<(&QueryValue, usize)> = vec![(value, 1)];
        while let Some((v, h)) = stack.pop() {
            if h > cap {
                return false;
            }
            match v {
                QueryValue::Map(map) => {
                    for child in map.values() {
                        stack.push((child, h + 1));
                    }
                }
                QueryValue::List(arr) => {
                    for child in arr {
                        stack.push((child, h + 1));
                    }
                }
                _ => {}
            }
        }
        true
    }

    /// Iterative (explicit-stack) height check on a `SelectExpr` tree —
    /// mirrors [`Self::value_height_bounded`]: proves that
    /// `to_filter_value`'s recursive translation cannot overflow.
    fn select_expr_height_bounded(expr: &crate::read::SelectExpr, cap: usize) -> bool {
        let mut stack: Vec<(&SelectExpr, usize)> = vec![(expr, 1)];
        while let Some((e, h)) = stack.pop() {
            if h > cap {
                return false;
            }
            if let SelectExpr::Add { left, right }
            | SelectExpr::Sub { left, right }
            | SelectExpr::Mul { left, right }
            | SelectExpr::Div { left, right } = e
            {
                stack.push((left, h + 1));
                stack.push((right, h + 1));
            }
        }
        true
    }

    /// Calculate maximum dependency depth — iterative Kahn wave counting.
    /// The graph is a DAG by the time this runs (`detect_cycle` ran first),
    /// and the wave index of a node IS its dependency depth, so a long
    /// alias chain never drives native recursion.
    ///
    /// `deps` maps node → its DEPENDENCIES, so `in_degree[node]` = number of
    /// dependencies: the initial frontier is the no-dependency nodes and the
    /// wave walk must traverse the REVERSE adjacency (dep → dependents),
    /// exactly like [`Self::topological_sort`] below.
    fn calculate_max_depth(deps: &TMap<&str, TSet<&str>>) -> usize {
        let mut in_degree: TMap<&str, usize> = deps.iter().map(|(k, v)| (*k, v.len())).collect();

        // reverse_adj[dep] = list of nodes that depend on `dep`.
        let mut reverse_adj: TMap<&str, Vec<&str>> = deps.keys().map(|k| (*k, vec![])).collect();
        for (node, neighbors) in deps {
            for n in neighbors {
                if let Some(list) = reverse_adj.get_mut(n) {
                    list.push(node);
                }
            }
        }

        let mut frontier: Vec<&str> = in_degree
            .iter()
            .filter(|&(_, &d)| d == 0)
            .map(|(&k, _)| k)
            .collect();
        let mut waves = 0usize;
        while !frontier.is_empty() {
            waves += 1;
            let mut next: Vec<&str> = Vec::new();
            for node in &frontier {
                if let Some(dependents) = reverse_adj.get(*node) {
                    for n in dependents {
                        if let Some(deg) = in_degree.get_mut(n) {
                            *deg -= 1;
                            if *deg == 0 {
                                next.push(n);
                            }
                        }
                    }
                }
            }
            frontier = next;
        }
        waves.saturating_sub(1)
    }

    /// Compute maximum static body nesting depth with the shared visitor.
    /// Outer batch = 0; Batch and ForEach each add 1, even for empty bodies.
    fn max_nesting_depth_of_ops(queries: &TMap<String, QueryEntry>) -> usize {
        BatchTreeVisitor::new(queries)
            .filter_map(|visit| visit.body_depth())
            .max()
            .unwrap_or(0)
    }

    /// Topological sort into parallel stages using Kahn's algorithm — O(V+E).
    ///
    /// Each stage contains queries whose dependencies are all satisfied
    /// by previous stages. Ties within a stage are broken by insertion order
    /// (`order` slice) for deterministic output.
    fn topological_sort(deps: &TMap<&str, TSet<&str>>, order: &[&str]) -> Vec<Vec<String>> {
        // Build a position map for O(1) tie-breaking.
        let pos: TMap<&str, usize> = order.iter().enumerate().map(|(i, s)| (*s, i)).collect();

        // In this DAG an edge A→B means "A depends on B, so B must run first".
        // For Kahn's we need the *reverse* adjacency (B→A: "completing B unblocks A")
        // and in-degree[A] = number of A's dependencies.

        // in_degree[node] = how many deps the node still has outstanding.
        let mut in_degree: TMap<&str, usize> = deps.keys().map(|k| (*k, deps[k].len())).collect();

        // reverse_adj[dep] = list of nodes that depend on `dep`.
        let mut reverse_adj: TMap<&str, Vec<&str>> = deps.keys().map(|k| (*k, vec![])).collect();
        for (node, neighbors) in deps {
            for n in neighbors {
                if let Some(list) = reverse_adj.get_mut(n) {
                    list.push(node);
                }
            }
        }

        // Seed the queue with zero-in-degree nodes (no dependencies).
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter_map(|(&k, &v)| if v == 0 { Some(k) } else { None })
            .collect();

        let mut stages: Vec<Vec<String>> = Vec::with_capacity(8);

        while !queue.is_empty() {
            // Collect the current wave, sort by original insertion order.
            let mut wave: Vec<&str> = queue.drain(..).collect();
            wave.sort_by_key(|&s| pos.get(s).copied().unwrap_or(usize::MAX));

            // For each completed node, decrement in-degree of dependents.
            for &node in &wave {
                if let Some(dependents) = reverse_adj.get(node) {
                    for &dep_node in dependents {
                        if let Some(deg) = in_degree.get_mut(dep_node) {
                            *deg -= 1;
                            if *deg == 0 {
                                queue.push_back(dep_node);
                            }
                        }
                    }
                }
            }

            stages.push(wave.into_iter().map(str::to_string).collect());
        }

        stages
    }
}
