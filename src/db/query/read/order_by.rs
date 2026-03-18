//! OrderBy — ORDER BY clause and related types.

use serde::{Deserialize, Serialize};

use crate::db::query::filter::FieldPath;

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
            field: vec![field.into()],
            direction: OrderDirection::Asc,
            nulls: None,
        }
    }

    pub fn desc(field: impl Into<String>) -> Self {
        OrderByItem {
            field: vec![field.into()],
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
