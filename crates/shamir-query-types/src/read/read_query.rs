//! ReadQuery — complete read query definition.
//!
//! This is the main entry point for SDBQL SELECT queries.

use serde::{Deserialize, Serialize};

use super::{GroupBy, OrderBy, Pagination, Select};
use crate::filter::Filter;
use crate::TableRef;

/// Complete read query definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadQuery {
    /// Table to query (optionally qualified with repo)
    pub from: TableRef,
    /// What to select (fields, aggregations)
    #[serde(default = "default_select")]
    pub select: Select,
    /// WHERE filter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Filter>,
    /// GROUP BY clause
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<GroupBy>,
    /// ORDER BY clause
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<OrderBy>,
    /// Pagination (LIMIT/OFFSET or page-based)
    #[serde(default, skip_serializing_if = "Pagination::is_none")]
    pub pagination: Pagination,
    /// Whether to compute and return total count (expensive)
    #[serde(default, skip_serializing_if = "is_false")]
    pub count_total: bool,
}

fn default_select() -> Select {
    Select::all()
}

fn is_false(v: &bool) -> bool {
    !v
}

impl ReadQuery {
    /// Create a new query for the given table (default repo "main")
    pub fn new(table: impl Into<String>) -> Self {
        ReadQuery {
            from: TableRef::new(table),
            select: Select::all(),
            r#where: None,
            group_by: None,
            order_by: None,
            pagination: Pagination::None,
            count_total: false,
        }
    }

    /// Create a new query with explicit repo
    pub fn with_repo(repo: impl Into<String>, table: impl Into<String>) -> Self {
        ReadQuery {
            from: TableRef::with_repo(repo, table),
            select: Select::all(),
            r#where: None,
            group_by: None,
            order_by: None,
            pagination: Pagination::None,
            count_total: false,
        }
    }

    pub fn select(mut self, select: Select) -> Self {
        self.select = select;
        self
    }

    pub fn filter(mut self, filter: Filter) -> Self {
        self.r#where = Some(filter);
        self
    }

    pub fn group_by(mut self, group: GroupBy) -> Self {
        self.group_by = Some(group);
        self
    }

    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.order_by = Some(order);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        match &mut self.pagination {
            Pagination::LimitOffset { limit: l, .. } => *l = Some(limit),
            _ => {
                self.pagination = Pagination::LimitOffset {
                    limit: Some(limit),
                    offset: 0,
                };
            }
        }
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        match &mut self.pagination {
            Pagination::LimitOffset { offset: o, .. } => *o = offset,
            _ => {
                self.pagination = Pagination::LimitOffset {
                    limit: None,
                    offset,
                };
            }
        }
        self
    }

    pub fn pagination(mut self, pagination: Pagination) -> Self {
        self.pagination = pagination;
        self
    }

    pub fn count_total(mut self, count: bool) -> Self {
        self.count_total = count;
        self
    }
}
