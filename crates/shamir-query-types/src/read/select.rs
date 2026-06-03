//! Select types for query projections.

use serde::{Deserialize, Serialize};

use super::{AggFunc, AggregateField, SelectExpr};
use crate::filter::{FieldPath, FilterValue};

/// What to select/return from a query
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Select {
    /// Select items (fields, aggregations, expressions)
    pub items: Vec<SelectItem>,
    /// Return distinct results
    #[serde(default)]
    pub distinct: bool,
}

impl Select {
    /// Select all fields (SELECT *)
    pub fn all() -> Self {
        Select {
            items: vec![SelectItem::All],
            distinct: false,
        }
    }

    /// Select specific fields
    pub fn fields(fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Select {
            items: fields
                .into_iter()
                .map(|f| SelectItem::Field {
                    path: vec![f.into()],
                    alias: None,
                })
                .collect(),
            distinct: false,
        }
    }

    /// Add distinct modifier
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }
}

/// Single select item
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SelectItem {
    /// Select all fields (*)
    All,

    /// Select a field with optional alias
    Field {
        path: FieldPath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },

    /// Aggregation function
    Aggregate {
        func: AggFunc,
        field: AggregateField,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        #[serde(default)]
        distinct: bool,
    },

    /// Count all records
    CountAll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },

    /// Library aggregate dispatched by name through the funclib aggregate
    /// registry (`median`, `mode`, `stddev`, `variance`, `percentile`,
    /// `count_distinct`, `string_agg`, `array_agg`, …).
    ///
    /// Distinct from [`SelectItem::Aggregate`], whose `func` is the closed
    /// fast-path set (`Count/Sum/Avg/Min/Max`). This variant carries a plain
    /// (non-folder-qualified) aggregate name resolved at execution time.
    AggregateFn {
        /// Aggregate name (plain, e.g. `"median"`).
        name: String,
        field: AggregateField,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        #[serde(default)]
        distinct: bool,
    },

    /// Scalar (row-level) function call in the projection, dispatched by name
    /// through the funclib scalar registry (`strings/upper`, `math/abs`, …).
    /// `args` reuse the filter value model — `$ref` field references, literals,
    /// and nested `$fn` calls — and are resolved per record against that row.
    Function {
        /// Folder-qualified scalar function name (e.g. `"strings/upper"`).
        name: String,
        #[serde(default)]
        args: Vec<FilterValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },

    /// Expression (future: computed fields)
    #[serde(rename = "expr")]
    Expression {
        expr: SelectExpr,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
    },
}
