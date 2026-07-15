//! [`BatchPlan`] — topological execution plan with parallel stages.

use shamir_collections::{TMap, TSet};

use super::edge_kind::EdgeKind;

/// Execution plan with parallel stages.
///
/// The planner analyzes dependencies and creates stages where
/// each stage contains queries that can run in parallel.
///
/// # Example
///
/// For queries with dependencies:
/// - `users` (no deps)
/// - `products` (no deps)
/// - `orders` (depends on users, products)
/// - `stats` (depends on orders)
///
/// The plan would be:
/// ```text
/// stages: [[users, products], [orders], [stats]]
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BatchPlan {
    /// Stages: each stage contains queries that can run in parallel.
    pub stages: Vec<Vec<String>>,

    /// All aliases in order.
    pub aliases: Vec<String>,

    /// Dependency graph (alias -> dependencies).
    pub dependencies: TMap<String, TSet<String>>,

    /// Provenance of each dependency edge: alias -> dep_alias -> how the
    /// edge was declared (`after`, `$query`, or both).
    ///
    /// Kept alongside `dependencies` (rather than folded into a richer
    /// value type) so existing consumers of `dependencies` keep working
    /// unchanged — provenance is purely additive. Every entry in
    /// `dependencies[alias]` has a matching entry in
    /// `edge_provenance[alias]` for the same `dep_alias`.
    pub edge_provenance: TMap<String, TMap<String, EdgeKind>>,
}
