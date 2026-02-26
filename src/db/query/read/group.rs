//! Grouping, ordering, and pagination types.

use serde::{Deserialize, Serialize};

use crate::db::query::filter::{FieldPath, Filter};

/// GROUP BY clause
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupBy {
    /// Fields to group by
    pub fields: Vec<FieldPath>,
    /// HAVING filter (applied after grouping)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub having: Option<Filter>,
}

impl GroupBy {
    pub fn new(fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        GroupBy {
            fields: fields.into_iter().map(|f| f.into()).collect(),
            having: None,
        }
    }

    pub fn having(mut self, filter: Filter) -> Self {
        self.having = Some(filter);
        self
    }

    pub fn having_opt(mut self, filter: Option<Filter>) -> Self {
        self.having = filter;
        self
    }
}

/// ORDER BY clause
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderBy {
    pub items: Vec<OrderByItem>,
}

impl OrderBy {
    pub fn new(items: impl IntoIterator<Item = OrderByItem>) -> Self {
        OrderBy {
            items: items.into_iter().collect(),
        }
    }

    /// Order by single field ascending
    pub fn asc(field: impl Into<String>) -> Self {
        OrderBy {
            items: vec![OrderByItem::asc(field)],
        }
    }

    /// Order by single field descending
    pub fn desc(field: impl Into<String>) -> Self {
        OrderBy {
            items: vec![OrderByItem::desc(field)],
        }
    }
}

/// Single order by item
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderByItem {
    pub field: FieldPath,
    #[serde(default)]
    pub direction: OrderDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nulls: Option<NullsOrder>,
}

impl OrderByItem {
    pub fn asc(field: impl Into<String>) -> Self {
        OrderByItem {
            field: field.into(),
            direction: OrderDirection::Asc,
            nulls: None,
        }
    }

    pub fn desc(field: impl Into<String>) -> Self {
        OrderByItem {
            field: field.into(),
            direction: OrderDirection::Desc,
            nulls: None,
        }
    }

    pub fn nulls_first(mut self) -> Self {
        self.nulls = Some(NullsOrder::First);
        self
    }

    pub fn nulls_last(mut self) -> Self {
        self.nulls = Some(NullsOrder::Last);
        self
    }
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderDirection {
    #[default]
    Asc,
    Desc,
}

/// NULL ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NullsOrder {
    First,
    Last,
}

/// LIMIT and OFFSET
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitOffset {
    /// Maximum records to return
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Records to skip
    #[serde(default)]
    pub offset: u64,
}

impl LimitOffset {
    pub fn new(limit: impl Into<Option<u64>>) -> Self {
        LimitOffset {
            limit: limit.into(),
            offset: 0,
        }
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    pub fn no_limit() -> Self {
        LimitOffset {
            limit: None,
            offset: 0,
        }
    }
}

impl Default for LimitOffset {
    fn default() -> Self {
        Self::no_limit()
    }
}
