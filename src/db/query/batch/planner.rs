//! Batch query planner.
//!
//! Analyzes dependencies between queries and creates an execution plan
//! that maximizes parallelism while respecting dependencies.
//!
//! # How It Works
//!
//! 1. **Extract Dependencies**: Scans all filters for `$query` references
//! 2. **Validate**: Checks for duplicates, unknown aliases, and cycles
//! 3. **Calculate Depth**: Ensures dependency chain isn't too deep
//! 4. **Topological Sort**: Groups queries into parallel stages
//!
//! # Example
//!
//! ```text
//! Input queries: [users, products, orders, stats]
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

use crate::db::query::batch::{BatchError, BatchLimits, BatchPlan, NamedQuery};
use crate::db::query::filter::Filter;
use crate::db::query::read::Query;
use crate::types::common::{new_map, new_set, TMap, TSet};

/// Batch query planner.
///
/// Creates execution plans from named queries with automatic
/// dependency detection and parallel stage grouping.
///
/// # Example
///
/// ```rust,ignore
/// use shamir_db::db::query::batch::{BatchPlanner, BatchLimits, NamedQuery};
///
/// let queries = vec![
///     NamedQuery { alias: "users".into(), query: Query::new("users"), return_result: true },
///     NamedQuery { alias: "orders".into(), query: Query::new("orders"), return_result: true },
/// ];
///
/// let plan = BatchPlanner::plan(&queries, &BatchLimits::default())?;
/// println!("Stages: {:?}", plan.stages);
/// ```
pub struct BatchPlanner;

impl BatchPlanner {
    /// Create an execution plan from named queries.
    ///
    /// # Returns
    ///
    /// - `Ok(BatchPlan)` with stages for parallel execution
    /// - `Err(BatchError)` if validation fails
    ///
    /// # Errors
    ///
    /// - `TooManyQueries`: More queries than `limits.max_queries`
    /// - `DuplicateAlias`: Same alias used twice
    /// - `UnknownAlias`: Reference to non-existent alias
    /// - `CircularDependency`: Cycle in dependency graph
    /// - `TooDeep`: Dependency chain exceeds `limits.max_dependency_depth`
    pub fn plan(queries: &[NamedQuery], limits: &BatchLimits) -> Result<BatchPlan, BatchError> {
        // Check query count
        if queries.len() > limits.max_queries {
            return Err(BatchError::TooManyQueries {
                count: queries.len(),
                max: limits.max_queries,
            });
        }

        // Build alias set and check duplicates
        let mut aliases: TSet<String> = new_set();
        let mut alias_order: Vec<String> = Vec::new();

        for q in queries {
            if aliases.contains(&q.alias) {
                return Err(BatchError::DuplicateAlias {
                    alias: q.alias.clone(),
                });
            }
            aliases.insert(q.alias.clone());
            alias_order.push(q.alias.clone());
        }

        // Extract dependencies for each query
        let mut dependencies: TMap<String, TSet<String>> = new_map();

        for q in queries {
            let deps = Self::extract_dependencies(&q.query);

            // Validate all referenced aliases exist
            for dep in &deps {
                if !aliases.contains(dep) {
                    return Err(BatchError::UnknownAlias {
                        alias: dep.clone(),
                        referenced_by: q.alias.clone(),
                    });
                }
            }

            dependencies.insert(q.alias.clone(), deps);
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

    /// Extract all query references from a query.
    fn extract_dependencies(query: &Query) -> TSet<String> {
        let mut deps = new_set();

        if let Some(filter) = &query.r#where {
            Self::extract_deps_from_filter(filter, &mut deps);
        }

        deps
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
            | Filter::NotExists { .. } => {}
        }
    }

    /// Extract dependencies from a filter value.
    fn extract_deps_from_filter_value(
        value: &crate::db::query::filter::FilterValue,
        deps: &mut TSet<String>,
    ) {
        use crate::db::query::filter::FilterValue;

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

    /// Extract base alias from a string like "users[0].id" -> "users".
    fn extract_base_alias(s: &str) -> String {
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

    /// Topological sort into parallel stages.
    ///
    /// Each stage contains queries whose dependencies are all satisfied
    /// by previous stages.
    fn topological_sort(
        deps: &TMap<String, TSet<String>>,
        order: &[String],
    ) -> Vec<Vec<String>> {
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
