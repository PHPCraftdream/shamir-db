//! Query AST - complete read query definition.
//!
//! This is the main entry point for SDBQL SELECT queries.

use serde::{Deserialize, Serialize};

use super::{GroupBy, LimitOffset, OrderBy, Select};
use crate::db::query::filter::Filter;

/// Table or store identifier
pub type TableName = String;

/// Complete read query definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Table to query
    pub from: TableName,
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
    /// LIMIT and OFFSET
    #[serde(default, skip_serializing_if = "is_default_limit")]
    pub limit: LimitOffset,
}

fn default_select() -> Select {
    Select::all()
}

fn is_default_limit(lo: &LimitOffset) -> bool {
    lo.limit.is_none() && lo.offset == 0
}

impl Query {
    /// Create a new query for the given table
    pub fn new(table: impl Into<String>) -> Self {
        Query {
            from: table.into(),
            select: Select::all(),
            r#where: None,
            group_by: None,
            order_by: None,
            limit: LimitOffset::no_limit(),
        }
    }

    /// Set select clause
    pub fn select(mut self, select: Select) -> Self {
        self.select = select;
        self
    }

    /// Set WHERE filter
    pub fn filter(mut self, filter: Filter) -> Self {
        self.r#where = Some(filter);
        self
    }

    /// Set GROUP BY
    pub fn group_by(mut self, group: GroupBy) -> Self {
        self.group_by = Some(group);
        self
    }

    /// Set ORDER BY
    pub fn order_by(mut self, order: OrderBy) -> Self {
        self.order_by = Some(order);
        self
    }

    /// Set LIMIT
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit.limit = Some(limit);
        self
    }

    /// Set OFFSET
    pub fn offset(mut self, offset: u64) -> Self {
        self.limit.offset = offset;
        self
    }
}

/// Query execution statistics
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryStats {
    /// Was an index used?
    pub index_used: Option<String>,
    /// Number of records scanned
    pub records_scanned: u64,
    /// Number of records returned
    pub records_returned: u64,
    /// Execution time in microseconds
    pub execution_time_us: u64,
}

/// Query result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// Result records (as JSON values)
    pub records: Vec<serde_json::Value>,
    /// Execution statistics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<QueryStats>,
    /// Has more results (for pagination)
    #[serde(default)]
    pub has_more: bool,
}
