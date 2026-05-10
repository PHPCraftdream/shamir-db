//! GroupBy — GROUP BY clause.

use serde::{Deserialize, Serialize};

use crate::query::filter::{FieldPath, Filter};

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
            fields: fields.into_iter().map(|f| vec![f.into()]).collect(),
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
