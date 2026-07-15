use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{
    At, GroupBy, OrderBy, OrderByItem, OrderDirection, Pagination, ReadQuery, Select, Temporal,
};
use shamir_query_types::TableRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::filter::{self as f};
use crate::val::IntoFieldPath;

use super::conds::{where_methods, Conds};
use super::into_select_item::IntoSelectItem;

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
    temporal: Temporal,
    with_version: bool,
    explain: bool,
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
            temporal: Temporal::Latest,
            with_version: false,
            explain: false,
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
            temporal: Temporal::Latest,
            with_version: false,
            explain: false,
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

    /// Keyset (seek) pagination: return up to `limit` rows ordered strictly
    /// after the seek tuple `key`.
    ///
    /// `key` is an ordered tuple with one [`QueryValue`] per ORDER BY column
    /// (typically the last row's values from the previous page). `limit`
    /// caps the page size (`None` = server default).
    ///
    /// The seek is **strictly greater** (ASC) or **strictly less** (DESC) than
    /// `key` — rows matching the seek tuple are excluded, so pages never
    /// overlap.
    ///
    /// Not to be confused with `Batch::after` (`crate::batch::Batch::after`)
    /// — that's inter-query DAG ordering (which batch entry runs after
    /// which); this is single-query result-set pagination. Two unrelated
    /// meanings sharing a method name on different types (`Query` vs
    /// `Batch`), kept as-is: the compiler already disambiguates by receiver
    /// type, and both docstrings cross-reference each other for humans.
    pub fn after(mut self, key: Vec<QueryValue>, limit: Option<u64>) -> Self {
        self.pagination = Pagination::after(key, limit);
        self
    }

    /// Keyset (seek) pagination WITH a record-id tie-breaker (task #537).
    ///
    /// Identical to [`after`](Self::after) but also carries `after_id` — the
    /// `_id` of the last row the client received on the previous page (read
    /// results surface `_id` for exactly this purpose). Passing it makes the
    /// seek resume STRICTLY after that specific row, so rows tied on the same
    /// ORDER BY value across a page boundary are no longer silently dropped.
    ///
    /// `after_id = None` is equivalent to `after(key, limit)` (the
    /// backward-compatible default — reproduces today's skip-all-ties
    /// behavior for callers that don't opt in).
    pub fn after_with_id(
        mut self,
        key: Vec<QueryValue>,
        limit: Option<u64>,
        after_id: Option<RecordId>,
    ) -> Self {
        self.pagination = Pagination::after_with_id(key, limit, after_id);
        self
    }

    /// Request total count computation (expensive).
    pub fn count_total(mut self, yes: bool) -> Self {
        self.count_total = yes;
        self
    }

    // ── temporal ────────────────────────────────────────────────

    /// Read the state of the table as it was at a specific version.
    ///
    /// Sets `temporal = AsOf { at: At::Version(version) }`.
    pub fn as_of_version(mut self, version: u64) -> Self {
        self.temporal = Temporal::AsOf {
            at: At::Version(version),
        };
        self
    }

    /// Read the state of the table as it was at a specific timestamp
    /// (epoch-milliseconds).
    ///
    /// Sets `temporal = AsOf { at: At::Timestamp(ts_millis) }`.
    pub fn as_of_timestamp(mut self, ts_millis: u64) -> Self {
        self.temporal = Temporal::AsOf {
            at: At::Timestamp(ts_millis),
        };
        self
    }

    /// Scan the full version history of the table (oldest → newest).
    ///
    /// Sets `temporal = History { from: None, to: None, limit: None,
    /// order: OrderDirection::Asc }`. Use [`history_range`](Query::history_range)
    /// for bounded or reordered scans.
    pub fn history(mut self) -> Self {
        self.temporal = Temporal::History {
            from: None,
            to: None,
            limit: None,
            order: OrderDirection::Asc,
        };
        self
    }

    /// Scan a bounded window of the version history.
    ///
    /// All four arguments are optional at the type level; pass `None` for
    /// open bounds / no cap / default order (`Asc`).
    ///
    /// ```rust
    /// use shamir_query_builder::{Query};
    /// use shamir_query_types::read::{At, OrderDirection};
    ///
    /// let rq = Query::from("events")
    ///     .history_range(
    ///         Some(At::Version(10)),
    ///         Some(At::Version(50)),
    ///         Some(100),
    ///         OrderDirection::Desc,
    ///     )
    ///     .build();
    /// ```
    pub fn history_range(
        mut self,
        from: Option<At>,
        to: Option<At>,
        limit: Option<u64>,
        order: OrderDirection,
    ) -> Self {
        self.temporal = Temporal::History {
            from,
            to,
            limit,
            order,
        };
        self
    }

    /// Include the record version number in query results.
    ///
    /// Sets `with_version = true`.
    pub fn with_version(mut self) -> Self {
        self.with_version = true;
        self
    }

    /// EXPLAIN / dry-run: run only the planner and return a plan preview
    /// without materialising any rows.
    pub fn explain(mut self) -> Self {
        self.explain = true;
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
            temporal: self.temporal,
            with_version: self.with_version,
            explain: self.explain,
        }
    }
}

impl From<Query> for ReadQuery {
    fn from(q: Query) -> Self {
        q.build()
    }
}
