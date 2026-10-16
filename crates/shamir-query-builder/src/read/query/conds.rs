use shamir_query_types::filter::{Filter, FilterValue};

use crate::filter::{self as f, FilterExt};
use crate::val::IntoFieldPath;

// Macro to avoid duplicating every where_* method on both Conds and Query.
macro_rules! where_methods {
    () => {
        // ── comparison ──────────────────────────────────────────

        /// AND-combine: `field == value`.
        pub fn where_eq(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::eq(field, value))
        }

        /// AND-combine: `field != value`.
        pub fn where_ne(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::ne(field, value))
        }

        /// AND-combine: `field > value`.
        pub fn where_gt(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::gt(field, value))
        }

        /// AND-combine: `field >= value`.
        pub fn where_gte(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::gte(field, value))
        }

        /// AND-combine: `field < value`.
        pub fn where_lt(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::lt(field, value))
        }

        /// AND-combine: `field <= value`.
        pub fn where_lte(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.and_filter(f::lte(field, value))
        }

        // ── set membership ──────────────────────────────────────

        /// AND-combine: `field IN (values...)`.
        pub fn where_in(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.and_filter(f::in_(field, values))
        }

        /// AND-combine: `field NOT IN (values...)`.
        pub fn where_not_in(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.and_filter(f::not_in(field, values))
        }

        // ── pattern matching ────────────────────────────────────

        /// AND-combine: `field LIKE pattern`.
        pub fn like(self, field: impl IntoFieldPath, pattern: impl Into<String>) -> Self {
            self.and_filter(f::like(field, pattern))
        }

        /// AND-combine: case-insensitive `LIKE`.
        pub fn ilike(self, field: impl IntoFieldPath, pattern: impl Into<String>) -> Self {
            self.and_filter(f::ilike(field, pattern))
        }

        /// AND-combine: `field ~ regex`.
        pub fn regex(self, field: impl IntoFieldPath, pattern: impl Into<String>) -> Self {
            self.and_filter(f::regex(field, pattern))
        }

        // ── null / existence ────────────────────────────────────

        /// AND-combine: `field IS NULL`.
        pub fn where_null(self, field: impl IntoFieldPath) -> Self {
            self.and_filter(f::is_null(field))
        }

        /// AND-combine: `field IS NOT NULL`.
        pub fn where_not_null(self, field: impl IntoFieldPath) -> Self {
            self.and_filter(f::is_not_null(field))
        }

        /// AND-combine: field exists.
        pub fn where_exists(self, field: impl IntoFieldPath) -> Self {
            self.and_filter(f::exists(field))
        }

        /// AND-combine: field does not exist.
        pub fn where_not_exists(self, field: impl IntoFieldPath) -> Self {
            self.and_filter(f::not_exists(field))
        }

        // ── containment ─────────────────────────────────────────

        /// AND-combine: array field contains `value`.
        pub fn where_contains(
            self,
            field: impl IntoFieldPath,
            value: impl Into<FilterValue>,
        ) -> Self {
            self.and_filter(f::contains(field, value))
        }

        /// AND-combine: array field contains any of `values`.
        pub fn where_contains_any(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.and_filter(f::contains_any(field, values))
        }

        /// AND-combine: array field contains all of `values`.
        pub fn where_contains_all(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.and_filter(f::contains_all(field, values))
        }

        // ── range ───────────────────────────────────────────────

        /// AND-combine: `from <= field <= to`.
        pub fn where_between(
            self,
            field: impl IntoFieldPath,
            from: impl Into<FilterValue>,
            to: impl Into<FilterValue>,
        ) -> Self {
            self.and_filter(f::between(field, from, to))
        }

        // ── full-text search ────────────────────────────────────

        /// AND-combine: full-text search.
        pub fn fts(
            self,
            field: impl IntoFieldPath,
            query: impl Into<String>,
            mode: impl Into<String>,
        ) -> Self {
            self.and_filter(f::fts(field, query, mode))
        }

        // ── drop-in filter ──────────────────────────────────────

        /// AND-combine a pre-built [`Filter`].
        pub fn where_(self, filter: Filter) -> Self {
            self.and_filter(filter)
        }

        // ── OR variants ─────────────────────────────────────────

        /// OR-combine: `field == value`.
        pub fn or_where_eq(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.or_filter(f::eq(field, value))
        }

        /// OR-combine: `field != value`.
        pub fn or_where_ne(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.or_filter(f::ne(field, value))
        }

        /// OR-combine: `field > value`.
        pub fn or_where_gt(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.or_filter(f::gt(field, value))
        }

        /// OR-combine: `field >= value`.
        pub fn or_where_gte(
            self,
            field: impl IntoFieldPath,
            value: impl Into<FilterValue>,
        ) -> Self {
            self.or_filter(f::gte(field, value))
        }

        /// OR-combine: `field < value`.
        pub fn or_where_lt(self, field: impl IntoFieldPath, value: impl Into<FilterValue>) -> Self {
            self.or_filter(f::lt(field, value))
        }

        /// OR-combine: `field <= value`.
        pub fn or_where_lte(
            self,
            field: impl IntoFieldPath,
            value: impl Into<FilterValue>,
        ) -> Self {
            self.or_filter(f::lte(field, value))
        }

        /// OR-combine: `field IN (values...)`.
        pub fn or_where_in(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.or_filter(f::in_(field, values))
        }

        /// OR-combine: `field NOT IN (values...)`.
        pub fn or_where_not_in(
            self,
            field: impl IntoFieldPath,
            values: impl IntoIterator<Item = impl Into<FilterValue>>,
        ) -> Self {
            self.or_filter(f::not_in(field, values))
        }

        /// OR-combine a pre-built [`Filter`].
        pub fn or_where_(self, filter: Filter) -> Self {
            self.or_filter(filter)
        }

        // ── nested groups ───────────────────────────────────────

        /// AND-combine a nested filter group built via a closure.
        ///
        /// The closure receives a fresh [`Conds`] accumulator; its
        /// resulting filter (if any) is AND-combined with the current
        /// state.
        pub fn where_group(self, builder: impl FnOnce(Conds) -> Conds) -> Self {
            let inner = builder(Conds::default());
            if let Some(f) = inner.into_filter() {
                self.and_filter(f)
            } else {
                self
            }
        }

        /// OR-combine a nested filter group built via a closure.
        pub fn where_group_or(self, builder: impl FnOnce(Conds) -> Conds) -> Self {
            let inner = builder(Conds::default());
            if let Some(f) = inner.into_filter() {
                self.or_filter(f)
            } else {
                self
            }
        }
    };
}

pub(super) use where_methods;

/// A standalone WHERE-clause accumulator.
///
/// Used both as the internal filter state of [`super::Query`] and as the
/// argument to `where_group` / `where_group_or` closures. Supports the
/// same `where_*` / `or_where_*` / `where_group*` API as `Query`.
#[derive(Debug, Clone, Default)]
pub struct Conds {
    pub(super) filter: Option<Filter>,
}

impl Conds {
    /// Create a new empty conditions accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume and return the accumulated filter (if any).
    pub fn into_filter(self) -> Option<Filter> {
        self.filter
    }

    /// AND-combine a filter into the accumulator (consumes + returns self).
    pub(super) fn and_filter(mut self, new: Filter) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(existing) => existing.and(new),
            None => new,
        });
        self
    }

    /// OR-combine a filter into the accumulator (consumes + returns self).
    pub(super) fn or_filter(mut self, new: Filter) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(existing) => existing.or(new),
            None => new,
        });
        self
    }

    where_methods!();
}
