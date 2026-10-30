//! [`BatchPlan`] — topological execution plan with parallel stages.

use shamir_collections::{TMap, TSet};

use super::edge_kind::EdgeKind;
use super::planner::{BatchPlanner, MAX_EXTRACT_DEPTH};
use crate::filter::{Filter, FilterValue};

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

impl BatchPlanner {
    /// #651/#663 defensive check: return `true` iff `filter` contains,
    /// anywhere in its tree (including nested inside any `FilterValue`
    /// operand it carries — e.g. a `$cond` embedded in `ValueCompare.left`),
    /// an OLD record-field-based comparison variant. These variants resolve
    /// a `FieldPath` against a REAL record — meaningless inside a `when`
    /// guard, which has no record (see `QueryRunner::resolve_skip`).
    ///
    /// Field-based variants (#651 + #663 Gap 1): `Eq`/`Ne`/`Gt`/`Gte`/`Lt`/
    /// `Lte`/`FieldEq`/`Like`/`ILike`/`Regex`/`In`/`NotIn`/`Contains`/
    /// `ContainsAny`/`ContainsAll`/`Between`/`Fts`/`VectorSimilarity`/
    /// `Computed` — every variant that carries a `field: FieldPath` and
    /// resolves it against a record, EXCEPT the presence-guard variants
    /// below.
    ///
    /// `IsNull`/`IsNotNull`/`Exists`/`NotExists` are intentionally EXCLUDED —
    /// they remain a legitimate presence-guard pattern against the synthetic
    /// record (ADR Decision 1). Do not add them here.
    ///
    /// `And`/`Or`/`Not` recurse structurally. `ValueCompare` is never itself
    /// flagged (it is genuinely record-free) but — like every other variant
    /// that carries one or more `FilterValue` operands — its operands are
    /// checked via [`Self::filter_value_contains_field_based_comparison`]
    /// (#663 Gap 2), since a `FilterValue::Cond` operand can embed its own
    /// field-based `condition`. The already-field-based variants below are
    /// unconditionally `true` regardless of their operands, so their
    /// `value`/`values`/`from`/`to`/`expr_args` are not separately walked —
    /// there is no additional signal a `true`-anyway variant could add.
    /// Depth parameter: over-deep trees return `true` (conservatively
    /// invalid) instead of recursing further — the planner rejects any
    /// filter deeper than [`MAX_EXTRACT_DEPTH`] anyway, so treating
    /// un-walkable depth as "contains a field-based comparison" only ever
    /// converts an eventual `NestingTooDeep` rejection into a slightly
    /// earlier guard rejection, never changes the verdict for legal depths.
    pub(super) fn contains_field_based_comparison(filter: &Filter, depth: usize) -> bool {
        // Unbounded recursion would overflow the native stack before the
        // planner's own depth-bounded extraction runs — cap the walk.
        if depth > MAX_EXTRACT_DEPTH {
            return true;
        }
        match filter {
            Filter::Eq { .. }
            | Filter::Ne { .. }
            | Filter::Gt { .. }
            | Filter::Gte { .. }
            | Filter::Lt { .. }
            | Filter::Lte { .. }
            | Filter::FieldEq { .. }
            | Filter::Contains { .. }
            | Filter::Like { .. }
            | Filter::ILike { .. }
            | Filter::Regex { .. }
            | Filter::Fts { .. }
            | Filter::VectorSimilarity { .. }
            | Filter::In { .. }
            | Filter::NotIn { .. }
            | Filter::ContainsAny { .. }
            | Filter::ContainsAll { .. }
            | Filter::Between { .. }
            | Filter::Computed { .. } => true,
            Filter::And { filters } | Filter::Or { filters } => filters
                .iter()
                .any(|f| Self::contains_field_based_comparison(f, depth + 1)),
            Filter::Not { filter } => Self::contains_field_based_comparison(filter, depth + 1),
            Filter::ValueCompare { left, right, .. } => {
                Self::filter_value_contains_field_based_comparison(left, depth + 1)
                    || Self::filter_value_contains_field_based_comparison(right, depth + 1)
            }
            Filter::IsNull { .. }
            | Filter::IsNotNull { .. }
            | Filter::Exists { .. }
            | Filter::NotExists { .. } => false,
        }
    }

    /// #663 Gap 2: return `true` iff `fv` — a `FilterValue` operand slot on
    /// some `Filter` variant — contains, anywhere inside it, a nested
    /// field-based comparison. This closes the gap where
    /// [`Self::contains_field_based_comparison`] only walked the
    /// STRUCTURAL `Filter` tree (`And`/`Or`/`Not`) and never looked inside
    /// the `FilterValue` operands every comparison variant carries — so a
    /// `FilterValue::Cond` embedding its own field-based `condition` (e.g.
    /// `Filter::ValueCompare { left: Cond{ if: Filter::Like{..}, .. }, .. }`)
    /// sailed through undetected.
    ///
    /// `QueryRef`/`Param`/`FieldRef`/literals are never themselves a
    /// field-based-comparison issue — a `$query`/`$param` reference is not a
    /// record-field lookup.
    pub(super) fn filter_value_contains_field_based_comparison(
        fv: &crate::filter::FilterValue,
        depth: usize,
    ) -> bool {
        // Same depth cap as [`Self::contains_field_based_comparison`] — see
        // its depth-parameter doc comment for why `true` is conservative.
        if depth > MAX_EXTRACT_DEPTH {
            return true;
        }
        match fv {
            FilterValue::Cond { cond } => {
                Self::contains_field_based_comparison(&cond.condition, depth + 1)
                    || Self::filter_value_contains_field_based_comparison(&cond.then, depth + 1)
                    || Self::filter_value_contains_field_based_comparison(&cond.or_else, depth + 1)
            }
            FilterValue::Expr { expr } => expr
                .args
                .iter()
                .any(|v| Self::filter_value_contains_field_based_comparison(v, depth + 1)),
            FilterValue::FnCall { call } => call
                .args()
                .iter()
                .any(|v| Self::filter_value_contains_field_based_comparison(v, depth + 1)),
            FilterValue::Array(items) => items
                .iter()
                .any(|v| Self::filter_value_contains_field_based_comparison(v, depth + 1)),
            _ => false,
        }
    }
}
