//! Logical combinators for [`Filter`]: free functions `and` / `or` / `not`
//! and the [`FilterExt`] chainable extension trait.

use shamir_query_types::filter::Filter;

// ── logical combinators (free functions) ─────────────────────────────

/// Combine filters with AND.
pub fn and(filters: impl IntoIterator<Item = Filter>) -> Filter {
    Filter::And {
        filters: filters.into_iter().collect(),
    }
}

/// Combine filters with OR.
pub fn or(filters: impl IntoIterator<Item = Filter>) -> Filter {
    Filter::Or {
        filters: filters.into_iter().collect(),
    }
}

/// Negate a filter.
pub fn not(filter: Filter) -> Filter {
    Filter::Not {
        filter: Box::new(filter),
    }
}

// ── FilterExt trait (chainable combinators with smart merge) ─────────

/// Chainable combinators for [`Filter`] with smart flattening.
///
/// `a.and(b)` flattens when `a` is already `Filter::And`; likewise for
/// `or`. This keeps the filter tree flat and avoids unnecessary nesting.
pub trait FilterExt {
    /// AND-combine with another filter (flattens existing `And` nodes).
    fn and(self, other: Filter) -> Filter;
    /// OR-combine with another filter (flattens existing `Or` nodes).
    fn or(self, other: Filter) -> Filter;
    /// Negate this filter (`Not`). Named `negate` to avoid clashing with
    /// the free function [`not`].
    fn negate(self) -> Filter;
}

impl FilterExt for Filter {
    fn and(self, other: Filter) -> Filter {
        match self {
            Filter::And { mut filters } => {
                filters.push(other);
                Filter::And { filters }
            }
            _ => Filter::And {
                filters: vec![self, other],
            },
        }
    }

    fn or(self, other: Filter) -> Filter {
        match self {
            Filter::Or { mut filters } => {
                filters.push(other);
                Filter::Or { filters }
            }
            _ => Filter::Or {
                filters: vec![self, other],
            },
        }
    }

    fn negate(self) -> Filter {
        Filter::Not {
            filter: Box::new(self),
        }
    }
}
