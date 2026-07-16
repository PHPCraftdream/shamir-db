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
use crate::batch::{BatchError, BatchLimits, BatchOp, BatchPlan, EdgeKind, QueryEntry};
use crate::filter::Filter;
use shamir_collections::{new_map, new_set, TFxSet, TMap, TSet};
use shamir_types::types::value::QueryValue;

// Maximum stack depth for the iterative nesting-depth walk (safety cap).
const NESTING_WALK_LIMIT: usize = 64;

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

        // Check static sub-batch nesting depth (iterative, bounded).
        let nesting = Self::max_nesting_depth_of_queries(queries);
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
        for entry in queries.values() {
            if let BatchOp::ForEach(fe) = &entry.op {
                if let crate::filter::FilterValue::Array(items) = &fe.over {
                    let iterations = items.len();
                    let body_len = fe.batch.queries.len();
                    let virtual_units = iterations.saturating_mul(body_len);
                    if virtual_units > limits.max_queries {
                        return Err(BatchError::TooManyQueries {
                            count: virtual_units,
                            max: limits.max_queries,
                        });
                    }
                }
            }
        }

        // Aliases are keys in the map (no duplicates possible)
        let aliases: TSet<String> = queries.keys().cloned().collect();
        let alias_order: Vec<String> = queries.keys().cloned().collect();

        // Extract dependencies for each query, tagging each edge with its
        // provenance (Explicit from `after`, DataFlow from `$query`, or
        // Both when the same alias is named by both sources).
        let mut dependencies: TMap<String, TSet<String>> = new_map();
        let mut edge_provenance: TMap<String, TMap<String, EdgeKind>> = new_map();

        for (alias, entry) in queries {
            let mut data_flow_deps = Self::extract_dependencies(&entry.op);

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
                if Self::contains_field_based_comparison(when_filter) {
                    return Err(BatchError::InvalidWhenFilter {
                        alias: alias.clone(),
                        message: "field-based comparisons are not meaningful inside `when` \
                            (no record exists) — use Filter::ValueCompare for value-vs-value \
                            comparisons instead"
                            .to_string(),
                    });
                }
                Self::extract_deps_from_filter(when_filter, &mut data_flow_deps);
            }

            let mut provenance: TMap<String, EdgeKind> = new_map();
            for dep in &data_flow_deps {
                provenance.insert(dep.clone(), EdgeKind::DataFlow);
            }

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

            let deps: TSet<String> = provenance.keys().cloned().collect();

            // Validate all referenced aliases exist
            for dep in &deps {
                if !aliases.contains(dep) {
                    return Err(BatchError::UnknownAlias {
                        alias: dep.clone(),
                        referenced_by: alias.clone(),
                    });
                }
            }

            dependencies.insert(alias.clone(), deps);
            edge_provenance.insert(alias.clone(), provenance);
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
            aliases: alias_order,
            dependencies,
            edge_provenance,
        })
    }

    /// Extract all query references from a batch operation.
    fn extract_dependencies(op: &BatchOp) -> TSet<String> {
        let mut deps = new_set();

        match op {
            BatchOp::Read(query) => {
                if let Some(filter) = &query.r#where {
                    Self::extract_deps_from_filter(filter, &mut deps);
                }
            }
            BatchOp::Update(update) => {
                if let Some(filter) = &update.where_clause {
                    Self::extract_deps_from_filter(filter, &mut deps);
                }
                Self::extract_deps_from_value(&update.set, &mut deps);
            }
            BatchOp::Set(set) => {
                Self::extract_deps_from_value(&set.key, &mut deps);
                Self::extract_deps_from_value(&set.value, &mut deps);
            }
            BatchOp::Delete(delete) => {
                Self::extract_deps_from_filter(&delete.where_clause, &mut deps);
            }
            BatchOp::Insert(insert) => {
                for value in &insert.values {
                    Self::extract_deps_from_value(value, &mut deps);
                }
            }
            // Stored-procedure call: scan positional params for `$query` refs
            // so a Call participates in the topological order (runs after any
            // query whose result it consumes as an argument).
            BatchOp::Call(call_op) => {
                for fv in &call_op.params {
                    Self::extract_deps_from_filter_value(fv, &mut deps);
                }
            }
            // Sub-batch: outer deps come exclusively from `bind` values.
            // Do NOT descend into the inner batch's queries — those are
            // planned recursively at execution time (P3).
            BatchOp::Batch(sub) => {
                for fv in sub.bind.values() {
                    Self::extract_deps_from_filter_value(fv, &mut deps);
                }
            }
            // ForEach loop (Epic04/B, #653): outer deps come exclusively
            // from `over` — the iteration source. Do NOT descend into the
            // loop body's queries — those are planned recursively at
            // execution time (K times), mirroring `Batch(sub)` above.
            BatchOp::ForEach(fe) => {
                Self::extract_deps_from_filter_value(&fe.over, &mut deps);
            }
            // Admin ops have no query dependencies
            _ => {}
        }

        deps
    }

    /// Extract dependencies from a write-row `QueryValue`
    /// (`InsertOp.values[i]`/`UpdateOp.set`/`SetOp.{key,value}`).
    ///
    /// #641 fix: this used to detect `$query` LOOSELY (`map.get("$query")`
    /// anywhere in a map, regardless of what else the map contained) and did
    /// NOT recurse into `$fn`'s args / `$cond`'s branches / `$expr`'s
    /// operands — so a `$query` ref nested inside one of those (e.g.
    /// `{"total": {"$expr": {"op": "add", "args": [{"$query": "@a"}, 1]}}}`)
    /// silently dropped out of the dependency graph (wrong topological
    /// order: the write could run BEFORE the alias it reads from). This now
    /// matches the precision AND recursion depth of
    /// [`Self::extract_deps_from_filter_value`] (the #642 fix for WHERE/
    /// `when` trees): a marker is detected with the exact reserved-key-map
    /// convention (mirroring `contains_param_ref`'s `is_marker_map` in
    /// `shamir-engine`'s `param_subst.rs`), then the marker map is decoded
    /// into a [`crate::filter::FilterValue`] via the same msgpack
    /// round-trip `param_subst.rs` uses to RESOLVE it at execution time —
    /// `FilterValue`'s wire encoding for `QueryRef`/`FnCall`/`Cond`/`Expr` IS
    /// exactly this reserved-key map shape — and delegated to the shared
    /// `extract_deps_from_filter_value` leaf extractor, so both extractors
    /// recurse identically forever after (one shared implementation, no
    /// drift).
    ///
    /// `$param` is intentionally NOT treated as a marker here: it carries no
    /// alias to depend on (it reads from the sub-batch's `bind` scope, not
    /// another query's result), so it contributes no dependency edge either
    /// way — same as the old code's silent no-op for it.
    fn extract_deps_from_value(value: &QueryValue, deps: &mut TSet<String>) {
        use crate::filter::FilterValue;

        match value {
            QueryValue::Map(map) => {
                // Reserved-key marker detection (mirrors
                // `contains_param_ref`'s `is_marker_map` in
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
                if is_query_fn_cond_expr_marker {
                    if let Some(fv) = rmp_serde::to_vec_named(value)
                        .ok()
                        .and_then(|bytes| rmp_serde::from_slice::<FilterValue>(&bytes).ok())
                    {
                        Self::extract_deps_from_filter_value(&fv, deps);
                        return;
                    }
                    // Malformed marker payload (fails to decode as a
                    // FilterValue): fall through to plain recursion below —
                    // execution-time resolution (`param_subst.rs`) is the
                    // authority that rejects malformed markers with a clear
                    // error; the planner's dependency pass stays best-effort.
                }
                // Recurse into nested objects (also covers `$param` markers
                // and non-marker maps).
                for v in map.values() {
                    Self::extract_deps_from_value(v, deps);
                }
            }
            QueryValue::List(arr) => {
                for v in arr {
                    Self::extract_deps_from_value(v, deps);
                }
            }
            _ => {}
        }
    }

    /// Extract dependencies from a filter.
    fn extract_deps_from_filter(filter: &Filter, deps: &mut TSet<String>) {
        match filter {
            Filter::Eq { value, .. }
            | Filter::Ne { value, .. }
            | Filter::Gt { value, .. }
            | Filter::Gte { value, .. }
            | Filter::Lt { value, .. }
            | Filter::Lte { value, .. }
            | Filter::Contains { value, .. } => {
                Self::extract_deps_from_filter_value(value, deps);
            }
            Filter::In { values, .. } | Filter::NotIn { values, .. } => {
                for v in values {
                    Self::extract_deps_from_filter_value(v, deps);
                }
            }
            Filter::Between { from, to, .. } => {
                Self::extract_deps_from_filter_value(from, deps);
                Self::extract_deps_from_filter_value(to, deps);
            }
            Filter::ContainsAny { values, .. } | Filter::ContainsAll { values, .. } => {
                for v in values {
                    Self::extract_deps_from_filter_value(v, deps);
                }
            }
            Filter::And { filters } | Filter::Or { filters } => {
                for f in filters {
                    Self::extract_deps_from_filter(f, deps);
                }
            }
            Filter::Not { filter } => {
                Self::extract_deps_from_filter(filter, deps);
            }
            Filter::FieldEq { value, .. } => {
                Self::extract_deps_from_filter_value(value, deps);
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
                Self::extract_deps_from_filter_value(value, deps);
                if let Some(args) = expr_args {
                    for v in args {
                        Self::extract_deps_from_filter_value(v, deps);
                    }
                }
            }
            Filter::ValueCompare { left, right, .. } => {
                Self::extract_deps_from_filter_value(left, deps);
                Self::extract_deps_from_filter_value(right, deps);
            }
        }
    }

    /// #651 defensive check: return `true` iff `filter` contains, anywhere
    /// in its tree, an OLD record-field-based comparison variant
    /// (`Eq`/`Ne`/`Gt`/`Gte`/`Lt`/`Lte`/`FieldEq`). These variants resolve a
    /// `FieldPath` against a REAL record — meaningless inside a `when` guard,
    /// which has no record (see `QueryRunner::resolve_skip`). `IsNull` /
    /// `IsNotNull` are intentionally EXCLUDED — they remain a legitimate
    /// presence-guard pattern against the synthetic record (ADR Decision 1).
    /// `And`/`Or`/`Not`/`ValueCompare` recurse/pass through without
    /// themselves being flagged.
    fn contains_field_based_comparison(filter: &Filter) -> bool {
        match filter {
            Filter::Eq { .. }
            | Filter::Ne { .. }
            | Filter::Gt { .. }
            | Filter::Gte { .. }
            | Filter::Lt { .. }
            | Filter::Lte { .. }
            | Filter::FieldEq { .. } => true,
            Filter::And { filters } | Filter::Or { filters } => {
                filters.iter().any(Self::contains_field_based_comparison)
            }
            Filter::Not { filter } => Self::contains_field_based_comparison(filter),
            _ => false,
        }
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
    fn extract_deps_from_filter_value(value: &crate::filter::FilterValue, deps: &mut TSet<String>) {
        use crate::filter::FilterValue;

        match value {
            FilterValue::Array(arr) => {
                for v in arr {
                    Self::extract_deps_from_filter_value(v, deps);
                }
            }
            FilterValue::QueryRef { alias, .. } => {
                let base_alias = Self::extract_base_alias(alias);
                deps.insert(base_alias);
            }
            FilterValue::Cond { cond } => {
                Self::extract_deps_from_filter(&cond.condition, deps);
                Self::extract_deps_from_filter_value(&cond.then, deps);
                Self::extract_deps_from_filter_value(&cond.or_else, deps);
            }
            FilterValue::Expr { expr } => {
                for arg in &expr.args {
                    Self::extract_deps_from_filter_value(arg, deps);
                }
            }
            FilterValue::FnCall { call } => {
                for arg in call.args() {
                    Self::extract_deps_from_filter_value(arg, deps);
                }
            }
            _ => {}
        }
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

    /// Detect cycle in dependency graph using DFS (white-gray-black algorithm).
    ///
    /// Uses borrow-based `HashSet<&str>` to avoid per-node `String` allocations
    /// in the DFS hot loop. Only allocates when a cycle is found (rare / error path).
    fn detect_cycle(deps: &TMap<String, TSet<String>>) -> Option<Vec<String>> {
        // Borrow-based sets — no allocations in the happy path.
        let mut white: TFxSet<&str> = deps.keys().map(String::as_str).collect();
        let mut gray: TFxSet<&str> = TFxSet::default();
        let mut black: TFxSet<&str> = TFxSet::default();

        fn dfs<'a>(
            node: &'a str,
            deps: &'a TMap<String, TSet<String>>,
            white: &mut TFxSet<&'a str>,
            gray: &mut TFxSet<&'a str>,
            black: &mut TFxSet<&'a str>,
            path: &mut Vec<&'a str>,
        ) -> Option<Vec<String>> {
            white.remove(node);
            gray.insert(node);
            path.push(node);

            if let Some(neighbors) = deps.get(node) {
                for neighbor in neighbors {
                    let nb: &str = neighbor.as_str();
                    if gray.contains(nb) {
                        // Found cycle — allocate only here (error path).
                        let cycle_start = path.iter().position(|&n| n == nb)?;
                        return Some(path[cycle_start..].iter().map(|&s| s.to_string()).collect());
                    }
                    if white.contains(nb) {
                        if let Some(cycle) = dfs(nb, deps, white, gray, black, path) {
                            return Some(cycle);
                        }
                    }
                }
            }

            gray.remove(node);
            black.insert(node);
            path.pop();
            None
        }

        // Collect keys once so we can iterate without borrowing `white` mutably.
        let all_nodes: Vec<&str> = deps.keys().map(String::as_str).collect();
        for node in all_nodes {
            if !white.contains(node) {
                continue;
            }
            let mut path: Vec<&str> = Vec::new();
            if let Some(cycle) = dfs(node, deps, &mut white, &mut gray, &mut black, &mut path) {
                return Some(cycle);
            }
        }

        None
    }

    /// Calculate maximum dependency depth.
    fn calculate_max_depth(deps: &TMap<String, TSet<String>>) -> usize {
        fn depth(
            node: &str,
            deps: &TMap<String, TSet<String>>,
            cache: &mut TMap<String, usize>,
        ) -> usize {
            if let Some(&d) = cache.get(node) {
                return d;
            }

            let d = if let Some(neighbors) = deps.get(node) {
                if neighbors.is_empty() {
                    0
                } else {
                    1 + neighbors
                        .iter()
                        .map(|n| depth(n, deps, cache))
                        .max()
                        .unwrap_or(0)
                }
            } else {
                0
            };

            cache.insert(node.to_string(), d);
            d
        }

        let mut cache: TMap<String, usize> = new_map();
        deps.keys()
            .map(|k| depth(k, deps, &mut cache))
            .max()
            .unwrap_or(0)
    }

    /// Compute the maximum static sub-batch nesting depth for a set of queries.
    ///
    /// Outer batch = depth 0.  Each `BatchOp::Batch` adds 1 for the batch it
    /// contains.  Uses an iterative worklist `(ops_slice, current_depth)` so
    /// a malicious deeply-nested payload cannot blow the call stack.
    ///
    /// Returns the deepest nesting found (0 when there are no sub-batches).
    fn max_nesting_depth_of_queries(queries: &TMap<String, QueryEntry>) -> usize {
        // Worklist: (list of BatchOp refs to inspect, depth assigned to
        // the batch that CONTAINS them).
        let ops: Vec<&BatchOp> = queries.values().map(|e| &e.op).collect();
        Self::max_nesting_depth_of_ops(&ops, 0)
    }

    fn max_nesting_depth_of_ops(ops: &[&BatchOp], current_depth: usize) -> usize {
        if current_depth >= NESTING_WALK_LIMIT {
            return current_depth;
        }

        let mut max = current_depth;
        for op in ops {
            // ForEach's body nests exactly like Batch(sub)'s — it is
            // planned once and re-planned recursively at execution time
            // (Epic04/B, #653), so its static nesting depth must be walked
            // the same way.
            let child_batch = match op {
                BatchOp::Batch(sub) => Some(&sub.batch),
                BatchOp::ForEach(fe) => Some(&fe.batch),
                _ => None,
            };
            if let Some(batch) = child_batch {
                let child_depth = current_depth + 1;
                let child_ops: Vec<&BatchOp> = batch.queries.values().map(|e| &e.op).collect();
                let d = Self::max_nesting_depth_of_ops(&child_ops, child_depth);
                if d > max {
                    max = d;
                }
            }
        }
        max
    }

    /// Topological sort into parallel stages using Kahn's algorithm — O(V+E).
    ///
    /// Each stage contains queries whose dependencies are all satisfied
    /// by previous stages. Ties within a stage are broken by insertion order
    /// (`order` slice) for deterministic output.
    fn topological_sort(deps: &TMap<String, TSet<String>>, order: &[String]) -> Vec<Vec<String>> {
        // Build a position map for O(1) tie-breaking.
        let pos: TMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();

        // In this DAG an edge A→B means "A depends on B, so B must run first".
        // For Kahn's we need the *reverse* adjacency (B→A: "completing B unblocks A")
        // and in-degree[A] = number of A's dependencies.

        // in_degree[node] = how many deps the node still has outstanding.
        let mut in_degree: TMap<&str, usize> =
            deps.keys().map(|k| (k.as_str(), deps[k].len())).collect();

        // reverse_adj[dep] = list of nodes that depend on `dep`.
        let mut reverse_adj: TMap<&str, Vec<&str>> =
            deps.keys().map(|k| (k.as_str(), vec![])).collect();
        for (node, neighbors) in deps {
            for n in neighbors {
                if let Some(list) = reverse_adj.get_mut(n.as_str()) {
                    list.push(node.as_str());
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
