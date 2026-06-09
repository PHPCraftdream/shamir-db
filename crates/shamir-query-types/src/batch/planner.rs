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
//! 4. **Topological Sort**: Groups queries into parallel stages
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

use crate::batch::{BatchError, BatchLimits, BatchOp, BatchPlan, QueryEntry};
use crate::filter::Filter;
use serde_json::Value;
use shamir_collections::{new_map, new_set, TMap, TSet};

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

        // Aliases are keys in the map (no duplicates possible)
        let aliases: TSet<String> = queries.keys().cloned().collect();
        let alias_order: Vec<String> = queries.keys().cloned().collect();

        // Extract dependencies for each query
        let mut dependencies: TMap<String, TSet<String>> = new_map();

        for (alias, entry) in queries {
            let mut deps = Self::extract_dependencies(&entry.op);

            // Merge explicit ordering dependencies from `after`.
            for raw in &entry.after {
                let base = Self::extract_base_alias(raw);
                deps.insert(base);
            }

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
            // Admin ops have no query dependencies
            _ => {}
        }

        deps
    }

    /// Extract dependencies from a JSON value.
    fn extract_deps_from_value(value: &Value, deps: &mut TSet<String>) {
        match value {
            Value::Object(map) => {
                // Check for $query reference
                if let Some(query_ref) = map.get("$query") {
                    if let Some(alias) = query_ref.as_str() {
                        let base_alias = Self::extract_base_alias(alias);
                        deps.insert(base_alias);
                    }
                }
                // Recurse into nested objects
                for v in map.values() {
                    Self::extract_deps_from_value(v, deps);
                }
            }
            Value::Array(arr) => {
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
        }
    }

    /// Extract dependencies from a filter value.
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
            _ => {}
        }
    }

    /// Extract base alias from a `$query` reference string like
    /// `"@users[0].id"` → `"users"`.
    ///
    /// The leading `@` is the explicit reference marker (per spec) and
    /// is stripped before lookup against the queries map (whose keys
    /// never carry `@`). Both forms are accepted on input — `@user` and
    /// bare `user` map to the same alias — but the canonical, documented
    /// form is **with** the `@`.
    fn extract_base_alias(s: &str) -> String {
        let s = s.strip_prefix('@').unwrap_or(s);
        s.find(['[', '.'])
            .map(|pos| s[..pos].to_string())
            .unwrap_or_else(|| s.to_string())
    }

    /// Detect cycle in dependency graph using DFS (white-gray-black algorithm).
    fn detect_cycle(deps: &TMap<String, TSet<String>>) -> Option<Vec<String>> {
        let mut white: TSet<String> = deps.keys().cloned().collect();
        let mut gray: TSet<String> = new_set();
        let mut black: TSet<String> = new_set();

        fn dfs(
            node: &str,
            deps: &TMap<String, TSet<String>>,
            white: &mut TSet<String>,
            gray: &mut TSet<String>,
            black: &mut TSet<String>,
            path: &mut Vec<String>,
        ) -> Option<Vec<String>> {
            white.shift_remove(node);
            gray.insert(node.to_string());
            path.push(node.to_string());

            if let Some(neighbors) = deps.get(node) {
                for neighbor in neighbors {
                    if gray.contains(neighbor) {
                        // Found cycle - extract it
                        let cycle_start = path.iter().position(|n| n == neighbor)?;
                        return Some(path[cycle_start..].to_vec());
                    }
                    if white.contains(neighbor) {
                        if let Some(cycle) = dfs(neighbor, deps, white, gray, black, path) {
                            return Some(cycle);
                        }
                    }
                }
            }

            gray.shift_remove(node);
            black.insert(node.to_string());
            path.pop();
            None
        }

        while let Some(node) = white.iter().next().cloned() {
            let mut path = Vec::new();
            if let Some(cycle) = dfs(&node, deps, &mut white, &mut gray, &mut black, &mut path) {
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
            if let BatchOp::Batch(sub) = op {
                let child_depth = current_depth + 1;
                let child_ops: Vec<&BatchOp> = sub.batch.queries.values().map(|e| &e.op).collect();
                let d = Self::max_nesting_depth_of_ops(&child_ops, child_depth);
                if d > max {
                    max = d;
                }
            }
        }
        max
    }

    /// Topological sort into parallel stages.
    ///
    /// Each stage contains queries whose dependencies are all satisfied
    /// by previous stages.
    fn topological_sort(deps: &TMap<String, TSet<String>>, order: &[String]) -> Vec<Vec<String>> {
        let mut stages: Vec<Vec<String>> = Vec::new();
        let mut completed: TSet<String> = new_set();
        let mut remaining: TSet<String> = deps.keys().cloned().collect();

        while !remaining.is_empty() {
            // Find all queries whose dependencies are satisfied
            let mut ready: Vec<String> = remaining
                .iter()
                .filter(|alias| {
                    deps.get(*alias)
                        .map(|d| d.is_subset(&completed))
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            if ready.is_empty() && !remaining.is_empty() {
                // Should not happen if we validated for cycles
                break;
            }

            // Sort by original order for deterministic output
            ready.sort_by_key(|a| order.iter().position(|x| x == a).unwrap_or(usize::MAX));

            for alias in &ready {
                remaining.shift_remove(alias);
                completed.insert(alias.clone());
            }

            stages.push(ready);
        }

        stages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::types::SubBatchOp;
    use crate::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry};
    use crate::filter::FilterValue;
    use crate::read::ReadQuery;
    use shamir_collections::{new_map, TMap};

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn read_entry(table: &str) -> QueryEntry {
        let q = ReadQuery {
            from: crate::TableRef::new(table),
            r#where: None,
            select: crate::read::Select::all(),
            order_by: None,
            pagination: crate::read::Pagination::default(),
            group_by: None,
            count_total: false,
            temporal: crate::read::Temporal::default(),
            with_version: false,
        };
        QueryEntry {
            op: BatchOp::Read(q),
            return_result: true,
            after: Vec::new(),
        }
    }

    fn sub_batch_entry(inner: BatchRequest, bind: TMap<String, FilterValue>) -> QueryEntry {
        QueryEntry {
            op: BatchOp::Batch(SubBatchOp { batch: inner, bind }),
            return_result: true,
            after: Vec::new(),
        }
    }

    fn empty_batch_request() -> BatchRequest {
        BatchRequest {
            id: serde_json::json!(1),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries: TMap::default(),
            return_all: true,
            return_only: None,
            limits: BatchLimits::default(),
        }
    }

    fn batch_request_with_queries(queries: TMap<String, QueryEntry>) -> BatchRequest {
        BatchRequest {
            id: serde_json::json!(1),
            name: None,
            transactional: false,
            isolation: None,
            durability: None,
            queries,
            return_all: true,
            return_only: None,
            limits: BatchLimits::default(),
        }
    }

    // -------------------------------------------------------------------------
    // Test 1: sub_batch_bind_query_ref_creates_dep
    // -------------------------------------------------------------------------

    #[test]
    fn sub_batch_bind_query_ref_creates_dep() {
        // outer batch:
        //   user  → ReadQuery (no deps)
        //   proc  → BatchOp::Batch with bind: { uid: $query @user[0].id }
        // Expected: "user" is in an earlier stage than "proc";
        //           dependencies["proc"] contains "user".
        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("user".to_string(), read_entry("users"));

        let mut bind: TMap<String, FilterValue> = new_map();
        bind.insert(
            "uid".to_string(),
            FilterValue::QueryRef {
                alias: "@user[0].id".to_string(),
                path: None,
            },
        );
        queries.insert(
            "proc".to_string(),
            sub_batch_entry(empty_batch_request(), bind),
        );

        let limits = BatchLimits::default();
        let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

        // proc must depend on user
        let proc_deps = plan
            .dependencies
            .get("proc")
            .expect("proc must have deps entry");
        assert!(
            proc_deps.contains("user"),
            "proc should depend on user, got {:?}",
            proc_deps
        );

        // user must be in an earlier stage
        let user_stage = plan
            .stages
            .iter()
            .position(|s| s.contains(&"user".to_string()))
            .expect("user must be in some stage");
        let proc_stage = plan
            .stages
            .iter()
            .position(|s| s.contains(&"proc".to_string()))
            .expect("proc must be in some stage");
        assert!(user_stage < proc_stage, "user must come before proc");
    }

    // -------------------------------------------------------------------------
    // Test 2: sub_batch_no_bind_no_dep
    // -------------------------------------------------------------------------

    #[test]
    fn sub_batch_no_bind_no_dep() {
        // A sub-batch with empty bind has no outer deps → can be stage 0.
        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("user".to_string(), read_entry("users"));
        queries.insert(
            "proc".to_string(),
            sub_batch_entry(empty_batch_request(), new_map()),
        );

        let limits = BatchLimits::default();
        let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

        let proc_deps = plan.dependencies.get("proc").expect("proc entry");
        assert!(proc_deps.is_empty(), "proc should have no outer deps");

        // Both should be in stage 0
        let stage0 = &plan.stages[0];
        assert!(
            stage0.contains(&"proc".to_string()),
            "proc with no deps should be in stage 0"
        );
    }

    // -------------------------------------------------------------------------
    // Test 3: nesting_depth_within_limit_ok
    // -------------------------------------------------------------------------

    #[test]
    fn nesting_depth_within_limit_ok() {
        // Depth is measured by max_nesting_depth_of_ops:
        // - outer queries map is depth 0 (the batch passed to plan()).
        // - A BatchOp::Batch at the top level is depth 1.
        // - A BatchOp::Batch one level deeper is depth 2.
        // - etc.
        //
        // To hit exactly depth == max_nesting_depth (4) we insert
        // (max - 1) = 3 wrapping levels below the top-level Batch op.
        let limits = BatchLimits {
            max_nesting_depth: 4,
            ..BatchLimits::default()
        };

        // Start with an empty leaf batch.
        let mut inner = empty_batch_request();
        // Wrap 3 times → chain of 3 nested Batch ops inside.
        for _ in 0..3 {
            let mut outer_queries: TMap<String, QueryEntry> = new_map();
            outer_queries.insert("inner".to_string(), sub_batch_entry(inner, new_map()));
            inner = batch_request_with_queries(outer_queries);
        }

        // Top-level entry "deep" is the 4th nesting level.
        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("deep".to_string(), sub_batch_entry(inner, new_map()));

        let result = BatchPlanner::plan(&queries, &limits);
        assert!(
            result.is_ok(),
            "nesting at limit should succeed: {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // Test 4: nesting_depth_exceeded_errors
    // -------------------------------------------------------------------------

    #[test]
    fn nesting_depth_exceeded_errors() {
        let limits = BatchLimits {
            max_nesting_depth: 2,
            ..BatchLimits::default()
        };

        // With limit=2, a top-level Batch op is depth 1; one level deeper is
        // depth 2 (== limit, ok). Wrapping one more level gives depth 3 (> limit).
        // So: 2 additional wrappings inside the top-level Batch op → depth 3.
        let mut inner = empty_batch_request();
        for _ in 0..2 {
            let mut outer_queries: TMap<String, QueryEntry> = new_map();
            outer_queries.insert("inner".to_string(), sub_batch_entry(inner, new_map()));
            inner = batch_request_with_queries(outer_queries);
        }

        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("deep".to_string(), sub_batch_entry(inner, new_map()));

        let result = BatchPlanner::plan(&queries, &limits);
        assert!(
            matches!(result, Err(BatchError::NestingTooDeep { .. })),
            "expected NestingTooDeep error, got {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // Test 5: param_value_not_treated_as_dep
    // -------------------------------------------------------------------------

    #[test]
    fn param_value_not_treated_as_dep() {
        // A sub-batch whose bind uses FilterValue::Param (inner-scope param)
        // should NOT create an outer-level dependency.
        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("user".to_string(), read_entry("users"));

        let mut bind: TMap<String, FilterValue> = new_map();
        bind.insert(
            "uid".to_string(),
            FilterValue::Param {
                name: "user_id".to_string(),
            },
        );
        queries.insert(
            "proc".to_string(),
            sub_batch_entry(empty_batch_request(), bind),
        );

        let limits = BatchLimits::default();
        let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

        let proc_deps = plan.dependencies.get("proc").expect("proc entry");
        assert!(
            proc_deps.is_empty(),
            "FilterValue::Param must not create outer dep, got {:?}",
            proc_deps
        );
    }
}
