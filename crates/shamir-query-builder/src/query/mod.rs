//! Fluent builder for [`ReadQuery`] — CodeIgniter Active Record style.
//!
//! [`Query`] is the headline API: chain projections, filters, grouping,
//! ordering, and pagination to produce a fully-formed
//! [`shamir_query_types::read::ReadQuery`] ready for the wire.
//!
//! # Quick example
//!
//! ```rust
//! use shamir_query_builder::{Query, filter, select, val::*};
//!
//! let rq = Query::from("users")
//!     .select(["id", "name", "age"])
//!     .where_eq("status", "active")
//!     .where_gt("age", 18)
//!     .where_in("role", ["admin", "mod"])
//!     .like("name", "Al%")
//!     .order_by_desc("age")
//!     .limit(20)
//!     .offset(40)
//!     .build();
//! ```
//!
//! Filters chain with AND by default (CodeIgniter semantics). Use
//! `or_where_*` for OR, and `where_group` / `where_group_or` for nested
//! parenthesised groups.

use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{
    GroupBy, OrderBy, OrderByItem, Pagination, ReadQuery, Select, SelectItem,
};
use shamir_query_types::TableRef;

use crate::filter::{self as f, FilterExt};
use crate::val::IntoFieldPath;

// ── IntoSelectItem ──────────────────────────────────────────────────

/// Anything convertible into a [`SelectItem`].
///
/// Implemented for `&str` / `String` (→ `SelectItem::Field`) and
/// `SelectItem` itself (passthrough), so both
/// `.select(["a", "b"])` and `.select([select::func(..)])` work.
pub trait IntoSelectItem {
    /// Convert into a select item.
    fn into_select_item(self) -> SelectItem;
}

impl IntoSelectItem for &str {
    fn into_select_item(self) -> SelectItem {
        crate::select::field(self)
    }
}

impl IntoSelectItem for String {
    fn into_select_item(self) -> SelectItem {
        crate::select::field(self)
    }
}

impl IntoSelectItem for SelectItem {
    fn into_select_item(self) -> SelectItem {
        self
    }
}

// ── Conds (standalone WHERE accumulator) ────────────────────────────

/// A standalone WHERE-clause accumulator.
///
/// Used both as the internal filter state of [`Query`] and as the
/// argument to `where_group` / `where_group_or` closures. Supports the
/// same `where_*` / `or_where_*` / `where_group*` API as `Query`.
#[derive(Debug, Clone, Default)]
pub struct Conds {
    filter: Option<Filter>,
}

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
    fn and_filter(mut self, new: Filter) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(existing) => existing.and(new),
            None => new,
        });
        self
    }

    /// OR-combine a filter into the accumulator (consumes + returns self).
    fn or_filter(mut self, new: Filter) -> Self {
        self.filter = Some(match self.filter.take() {
            Some(existing) => existing.or(new),
            None => new,
        });
        self
    }

    where_methods!();
}

// ── Query ───────────────────────────────────────────────────────────

/// Fluent builder for [`ReadQuery`].
///
/// Construct via [`Query::from`] or [`Query::with_repo`], chain
/// projections / filters / grouping / ordering / pagination, then call
/// [`.build()`](Query::build) (or convert via `Into<ReadQuery>`).
#[derive(Debug, Clone)]
pub struct Query {
    from: TableRef,
    select: Select,
    conds: Conds,
    group_by_fields: Vec<Vec<String>>,
    having: Option<Filter>,
    order_by_items: Vec<OrderByItem>,
    pagination: Pagination,
    count_total: bool,
}

impl Query {
    /// Start a query targeting `table` in the default repo.
    pub fn from(table: impl Into<String>) -> Self {
        Self {
            from: TableRef::new(table),
            select: Select::all(),
            conds: Conds::default(),
            group_by_fields: Vec::new(),
            having: None,
            order_by_items: Vec::new(),
            pagination: Pagination::None,
            count_total: false,
        }
    }

    /// Start a query targeting `table` in an explicit `repo`.
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            from: TableRef::with_repo(repo, table),
            select: Select::all(),
            conds: Conds::default(),
            group_by_fields: Vec::new(),
            having: None,
            order_by_items: Vec::new(),
            pagination: Pagination::None,
            count_total: false,
        }
    }

    // ── projection ──────────────────────────────────────────────

    /// Set the projection (replaces any previous select).
    ///
    /// Accepts an iterator of anything implementing [`IntoSelectItem`]:
    /// `&str` / `String` map to `SelectItem::Field`, and `SelectItem`
    /// passes through.
    pub fn select(mut self, items: impl IntoIterator<Item = impl IntoSelectItem>) -> Self {
        self.select = Select {
            items: items.into_iter().map(|i| i.into_select_item()).collect(),
            distinct: self.select.distinct,
        };
        self
    }

    /// Enable `SELECT DISTINCT`.
    pub fn distinct(mut self) -> Self {
        self.select.distinct = true;
        self
    }

    // ── WHERE (delegated to embedded Conds) ─────────────────────

    /// Internal: AND-combine a filter.
    fn and_filter(mut self, new: Filter) -> Self {
        self.conds = self.conds.and_filter(new);
        self
    }

    /// Internal: OR-combine a filter.
    fn or_filter(mut self, new: Filter) -> Self {
        self.conds = self.conds.or_filter(new);
        self
    }

    where_methods!();

    // ── GROUP BY / HAVING ───────────────────────────────────────

    /// Append one field to the GROUP BY clause.
    pub fn group_by(mut self, field: impl IntoFieldPath) -> Self {
        self.group_by_fields.push(field.into_field_path());
        self
    }

    /// Append many fields to the GROUP BY clause.
    pub fn group_by_many(mut self, fields: impl IntoIterator<Item = impl IntoFieldPath>) -> Self {
        for f in fields {
            self.group_by_fields.push(f.into_field_path());
        }
        self
    }

    /// Set the HAVING filter (requires GROUP BY for meaningful queries,
    /// but attaches even without — the engine may accept it).
    pub fn having(mut self, filter: Filter) -> Self {
        self.having = Some(filter);
        self
    }

    // ── ORDER BY ────────────────────────────────────────────────

    /// Append an ascending order-by item.
    pub fn order_by_asc(mut self, field: impl Into<String>) -> Self {
        self.order_by_items.push(OrderByItem::asc(field));
        self
    }

    /// Append a descending order-by item.
    pub fn order_by_desc(mut self, field: impl Into<String>) -> Self {
        self.order_by_items.push(OrderByItem::desc(field));
        self
    }

    /// Append a fully-specified order-by item (for nulls ordering etc.).
    pub fn order_by(mut self, item: OrderByItem) -> Self {
        self.order_by_items.push(item);
        self
    }

    // ── pagination ──────────────────────────────────────────────

    /// Set the maximum number of records to return (LIMIT).
    pub fn limit(mut self, n: u64) -> Self {
        match &mut self.pagination {
            Pagination::LimitOffset { limit, .. } => *limit = Some(n),
            _ => {
                self.pagination = Pagination::LimitOffset {
                    limit: Some(n),
                    offset: 0,
                };
            }
        }
        self
    }

    /// Set the number of records to skip (OFFSET).
    pub fn offset(mut self, n: u64) -> Self {
        match &mut self.pagination {
            Pagination::LimitOffset { offset, .. } => *offset = n,
            _ => {
                self.pagination = Pagination::LimitOffset {
                    limit: None,
                    offset: n,
                };
            }
        }
        self
    }

    /// Set page-based pagination.
    pub fn page(mut self, page: u64, size: u64) -> Self {
        self.pagination = Pagination::Page {
            page,
            page_size: size,
        };
        self
    }

    /// Request total count computation (expensive).
    pub fn count_total(mut self, yes: bool) -> Self {
        self.count_total = yes;
        self
    }

    // ── terminal ────────────────────────────────────────────────

    /// Consume the builder and produce the wire-ready [`ReadQuery`].
    pub fn build(self) -> ReadQuery {
        let group_by = if !self.group_by_fields.is_empty() || self.having.is_some() {
            Some(GroupBy {
                fields: self.group_by_fields,
                having: self.having,
            })
        } else {
            None
        };

        let order_by = if self.order_by_items.is_empty() {
            None
        } else {
            Some(OrderBy {
                items: self.order_by_items,
            })
        };

        ReadQuery {
            from: self.from,
            select: self.select,
            r#where: self.conds.into_filter(),
            group_by,
            order_by,
            pagination: self.pagination,
            count_total: self.count_total,
        }
    }
}

impl From<Query> for ReadQuery {
    fn from(q: Query) -> Self {
        q.build()
    }
}

#[cfg(test)]
mod tests;
