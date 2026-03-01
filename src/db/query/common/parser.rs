//! Common query parsing utilities shared across all query types.
//!
//! This module provides parsers for query components that are used by
//! multiple query types (SELECT, UPDATE, DELETE, etc.).

use crate::db::query::filter::{Cond, FilterExpr, FilterExprOp, Filter, FilterValue, FnCall};
use crate::db::query::read::{
    AggFunc, AggregateField, SelectExpr, SelectExprValue, GroupBy, Pagination, OrderBy,
    OrderByItem,
};
use crate::types::value::{QueryValue, Value};

/// Error during query parsing
#[derive(Debug, Clone, PartialEq)]
pub enum QueryParseError {
    /// Expected an object/map
    NotAnObject,
    /// Missing required field
    MissingField(&'static str),
    /// Invalid type for field
    InvalidType(&'static str, &'static str),
    /// Unknown enum variant
    UnknownType(String),
    /// Unknown aggregate function
    UnknownAggregateFunction(String),
    /// Unknown filter operator
    UnknownFilterOp(String),
    /// Invalid field value
    InvalidField(&'static str, &'static str),
}

impl std::fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryParseError::NotAnObject => write!(f, "Expected object/map"),
            QueryParseError::MissingField(field) => write!(f, "Missing required field: {}", field),
            QueryParseError::InvalidType(field, expected) => {
                write!(f, "Invalid type for '{}', expected: {}", field, expected)
            }
            QueryParseError::UnknownType(t) => write!(f, "Unknown type: {}", t),
            QueryParseError::UnknownAggregateFunction(func) => {
                write!(f, "Unknown aggregate function: {}", func)
            }
            QueryParseError::UnknownFilterOp(op) => write!(f, "Unknown filter operator: {}", op),
            QueryParseError::InvalidField(field, expected) => {
                write!(f, "Invalid value for '{}', expected: {}", field, expected)
            }
        }
    }
}

impl std::error::Error for QueryParseError {}

/// Parse Expr from QueryValue (placeholder for future)
pub fn expr_from_value(value: &QueryValue) -> Result<SelectExpr, QueryParseError> {
    match value {
        Value::Map(map) => {
            let type_str = match map.get(&"type".to_string()) {
                Some(Value::Str(s)) => s.as_str(),
                _ => return Err(QueryParseError::MissingField("expr.type")),
            };

            match type_str {
                "field" => {
                    let name = match map.get(&"name".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("expr.name")),
                    };
                    Ok(SelectExpr::Field { path: name })
                }
                "literal" => {
                    let value = map
                        .get(&"value".to_string())
                        .cloned()
                        .ok_or(QueryParseError::MissingField("expr.value"))?;
                    let expr_value = expr_value_from_value(&value)?;
                    Ok(SelectExpr::Literal { value: expr_value })
                }
                _ => Err(QueryParseError::UnknownType(type_str.to_string())),
            }
        }
        _ => Err(QueryParseError::InvalidType("expr", "object")),
    }
}

/// Parse SelectExprValue from QueryValue
pub fn expr_value_from_value(value: &QueryValue) -> Result<SelectExprValue, QueryParseError> {
    match value {
        Value::Null => Ok(SelectExprValue::Null),
        Value::Bool(b) => Ok(SelectExprValue::Bool(*b)),
        Value::Int(i) => Ok(SelectExprValue::Int(*i)),
        Value::F64(f) => Ok(SelectExprValue::Float(*f)),
        Value::Str(s) => Ok(SelectExprValue::String(s.clone())),
        _ => Err(QueryParseError::InvalidType("expr.value", "primitive")),
    }
}

/// Parse Filter from QueryValue
pub fn filter_from_value(value: &QueryValue) -> Result<Filter, QueryParseError> {
    match value {
        Value::Map(map) => {
            let op = match map.get(&"op".to_string()) {
                Some(Value::Str(s)) => s.as_str(),
                _ => return Err(QueryParseError::MissingField("filter.op")),
            };

            match op {
                "eq" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Eq { field, value })
                }
                "ne" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Ne { field, value })
                }
                "gt" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Gt { field, value })
                }
                "gte" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Gte { field, value })
                }
                "lt" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Lt { field, value })
                }
                "lte" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let value = match map.get(&"value".to_string()) {
                        Some(v) => filter_value_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.value")),
                    };
                    Ok(Filter::Lte { field, value })
                }
                "and" => {
                    let filters = match map.get(&"filters".to_string()) {
                        Some(Value::List(list)) => list,
                        None => return Err(QueryParseError::MissingField("filter.filters")),
                        Some(_) => return Err(QueryParseError::InvalidType("filters", "array")),
                    };
                    let parsed = filters
                        .iter()
                        .map(filter_from_value)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Filter::And { filters: parsed })
                }
                "or" => {
                    let filters = match map.get(&"filters".to_string()) {
                        Some(Value::List(list)) => list,
                        None => return Err(QueryParseError::MissingField("filter.filters")),
                        Some(_) => return Err(QueryParseError::InvalidType("filters", "array")),
                    };
                    let parsed = filters
                        .iter()
                        .map(filter_from_value)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Filter::Or { filters: parsed })
                }
                "not" => {
                    let filter = match map.get(&"filter".to_string()) {
                        Some(v @ (Value::Map(_) | Value::Null)) => filter_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("filter.filter")),
                        Some(_) => return Err(QueryParseError::InvalidType("filter", "object")),
                    };
                    Ok(Filter::Not {
                        filter: Box::new(filter),
                    })
                }
                "in" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let values = match map.get(&"values".to_string()) {
                        Some(Value::List(list)) => list
                            .iter()
                            .map(filter_value_from_value)
                            .collect::<Result<Vec<_>, _>>()?,
                        // Single QueryRef value — wrap in vec
                        Some(v @ Value::Map(_)) => vec![filter_value_from_value(v)?],
                        None => return Err(QueryParseError::MissingField("filter.values")),
                        Some(_) => {
                            return Err(QueryParseError::InvalidType("values", "array or $query"))
                        }
                    };
                    Ok(Filter::In { field, values })
                }
                "not_in" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    let values = match map.get(&"values".to_string()) {
                        Some(Value::List(list)) => list
                            .iter()
                            .map(filter_value_from_value)
                            .collect::<Result<Vec<_>, _>>()?,
                        Some(v @ Value::Map(_)) => vec![filter_value_from_value(v)?],
                        None => return Err(QueryParseError::MissingField("filter.values")),
                        Some(_) => {
                            return Err(QueryParseError::InvalidType("values", "array or $query"))
                        }
                    };
                    Ok(Filter::NotIn { field, values })
                }
                "is_null" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    Ok(Filter::IsNull { field })
                }
                "is_not_null" => {
                    let field = match map.get(&"field".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("filter.field")),
                    };
                    Ok(Filter::IsNotNull { field })
                }
                _ => Err(QueryParseError::UnknownFilterOp(op.to_string())),
            }
        }
        _ => Err(QueryParseError::InvalidType("filter", "object")),
    }
}

/// Parse FilterValue from QueryValue
pub fn filter_value_from_value(value: &QueryValue) -> Result<FilterValue, QueryParseError> {
    match value {
        Value::Null => Ok(FilterValue::Null),
        Value::Bool(b) => Ok(FilterValue::Bool(*b)),
        Value::Int(i) => Ok(FilterValue::Int(*i)),
        Value::F64(f) => Ok(FilterValue::Float(*f)),
        Value::Str(s) => Ok(FilterValue::String(s.clone())),
        Value::List(list) => {
            let parsed = list
                .iter()
                .map(filter_value_from_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(FilterValue::Array(parsed))
        }
        Value::Map(map) => {
            // Check for field reference: { "$ref": "path.to.field" }
            if let Some(Value::Str(path)) = map.get(&"$ref".to_string()) {
                return Ok(FilterValue::FieldRef { path: path.clone() });
            }

            // Check for query reference: { "$query": "@alias.path" }
            if let Some(Value::Str(query_ref)) = map.get(&"$query".to_string()) {
                // Parse @alias[...].path format
                let ref_str = query_ref.trim();
                if !ref_str.starts_with('@') {
                    return Err(QueryParseError::InvalidField("$query", "must start with @"));
                }

                let rest = &ref_str[1..];
                if rest.is_empty() {
                    return Err(QueryParseError::InvalidField("$query", "missing alias"));
                }

                // Find where alias ends
                let pos = rest.find(['[', '.']).unwrap_or(rest.len());
                let alias = &rest[..pos];
                let path = if pos < rest.len() {
                    Some(rest[pos..].to_string())
                } else {
                    None
                };

                return Ok(FilterValue::QueryRef {
                    alias: alias.to_string(),
                    path,
                });
            }

            // Check for function call: { "$fn": "NOW" } or { "$fn": { "name": "COALESCE", "args": [...] } }
            if let Some(fn_val) = map.get(&"$fn".to_string()) {
                let fn_call = fn_call_from_value(fn_val)?;
                return Ok(FilterValue::FnCall { call: fn_call });
            }

            // Check for expression: { "$expr": { "op": "add", "args": [...] } }
            if let Some(expr_val) = map.get(&"$expr".to_string()) {
                let expr = expr_filter_from_value(expr_val)?;
                return Ok(FilterValue::Expr { expr });
            }

            // Check for conditional: { "$cond": { "if": ..., "then": ..., "else": ... } }
            if let Some(cond_val) = map.get(&"$cond".to_string()) {
                let cond = cond_from_value(cond_val)?;
                return Ok(FilterValue::Cond {
                    cond: Box::new(cond),
                });
            }

            Err(QueryParseError::InvalidType(
                "filter.value",
                "primitive, $ref, $query, $fn, $expr, or $cond",
            ))
        }
        _ => Err(QueryParseError::InvalidType("filter.value", "primitive")),
    }
}

/// Parse FnCall from QueryValue
fn fn_call_from_value(value: &QueryValue) -> Result<FnCall, QueryParseError> {
    match value {
        // Simple form: { "$fn": "NOW" }
        Value::Str(name) => Ok(FnCall::simple(name)),
        // Complex form: { "$fn": { "name": "COALESCE", "args": [...] } }
        Value::Map(map) => {
            let name = match map.get(&"name".to_string()) {
                Some(Value::Str(s)) => s.clone(),
                _ => return Err(QueryParseError::MissingField("$fn.name")),
            };
            let args = match map.get(&"args".to_string()) {
                Some(Value::List(list)) => list
                    .iter()
                    .map(filter_value_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
                _ => Vec::new(),
            };
            Ok(FnCall::complex(name, args))
        }
        _ => Err(QueryParseError::InvalidType("$fn", "string or object")),
    }
}

/// Parse FilterExpr (filter expression) from QueryValue
fn expr_filter_from_value(value: &QueryValue) -> Result<FilterExpr, QueryParseError> {
    match value {
        Value::Map(map) => {
            let op = match map.get(&"op".to_string()) {
                Some(Value::Str(s)) => expr_op_from_str(s)?,
                _ => return Err(QueryParseError::MissingField("$expr.op")),
            };
            let args = match map.get(&"args".to_string()) {
                Some(Value::List(list)) => list
                    .iter()
                    .map(filter_value_from_value)
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(QueryParseError::MissingField("$expr.args")),
            };
            Ok(FilterExpr::new(op, args))
        }
        _ => Err(QueryParseError::InvalidType("$expr", "object")),
    }
}

/// Parse FilterExprOp from string
fn expr_op_from_str(s: &str) -> Result<FilterExprOp, QueryParseError> {
    match s {
        // Math
        "add" => Ok(FilterExprOp::Add),
        "sub" => Ok(FilterExprOp::Sub),
        "mul" => Ok(FilterExprOp::Mul),
        "div" => Ok(FilterExprOp::Div),
        "mod" => Ok(FilterExprOp::Mod),
        "neg" => Ok(FilterExprOp::Neg),
        // String
        "concat" => Ok(FilterExprOp::Concat),
        "lower" => Ok(FilterExprOp::Lower),
        "upper" => Ok(FilterExprOp::Upper),
        "trim" => Ok(FilterExprOp::Trim),
        "length" => Ok(FilterExprOp::Length),
        // Logic
        "and" => Ok(FilterExprOp::And),
        "or" => Ok(FilterExprOp::Or),
        "not" => Ok(FilterExprOp::Not),
        // Comparison
        "eq" => Ok(FilterExprOp::Eq),
        "ne" => Ok(FilterExprOp::Ne),
        "gt" => Ok(FilterExprOp::Gt),
        "gte" => Ok(FilterExprOp::Gte),
        "lt" => Ok(FilterExprOp::Lt),
        "lte" => Ok(FilterExprOp::Lte),
        _ => Err(QueryParseError::UnknownType(format!("expr.op: {}", s))),
    }
}

/// Parse Cond from QueryValue
fn cond_from_value(value: &QueryValue) -> Result<Cond, QueryParseError> {
    match value {
        Value::Map(map) => {
            let condition = match map.get(&"if".to_string()) {
                Some(v) => filter_from_value(v)?,
                _ => return Err(QueryParseError::MissingField("$cond.if")),
            };
            let then = match map.get(&"then".to_string()) {
                Some(v) => filter_value_from_value(v)?,
                _ => return Err(QueryParseError::MissingField("$cond.then")),
            };
            let or_else = match map.get(&"else".to_string()) {
                Some(v) => filter_value_from_value(v)?,
                _ => return Err(QueryParseError::MissingField("$cond.else")),
            };
            Ok(Cond::new(condition, then, or_else))
        }
        _ => Err(QueryParseError::InvalidType("$cond", "object")),
    }
}

/// Parse GroupBy from QueryValue
pub fn group_by_from_value(value: &QueryValue) -> Result<GroupBy, QueryParseError> {
    match value {
        Value::Map(map) => {
            let fields = match map.get(&"fields".to_string()) {
                Some(Value::List(list)) => list
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                None => return Err(QueryParseError::MissingField("group_by.fields")),
                Some(_) => return Err(QueryParseError::InvalidType("fields", "array")),
            };

            let having = match map.get(&"having".to_string()) {
                Some(v @ (Value::Map(_) | Value::Null)) => Some(filter_from_value(v)?),
                None => None,
                Some(_) => return Err(QueryParseError::InvalidType("having", "filter")),
            };

            Ok(GroupBy::new(fields).having_opt(having))
        }
        _ => Err(QueryParseError::InvalidType("group_by", "object")),
    }
}

/// Parse OrderBy from QueryValue
pub fn order_by_from_value(value: &QueryValue) -> Result<OrderBy, QueryParseError> {
    match value {
        Value::Map(map) => {
            let items = match map.get(&"items".to_string()) {
                Some(Value::List(list)) => list,
                None => return Err(QueryParseError::MissingField("order_by.items")),
                Some(_) => return Err(QueryParseError::InvalidType("items", "array")),
            };

            let parsed = items
                .iter()
                .map(order_by_item_from_value)
                .collect::<Result<Vec<_>, _>>()?;

            Ok(OrderBy::new(parsed))
        }
        _ => Err(QueryParseError::InvalidType("order_by", "object")),
    }
}

/// Parse OrderByItem from QueryValue
pub fn order_by_item_from_value(value: &QueryValue) -> Result<OrderByItem, QueryParseError> {
    match value {
        Value::Map(map) => {
            let field = match map.get(&"field".to_string()) {
                Some(Value::Str(s)) => s.clone(),
                _ => return Err(QueryParseError::MissingField("order_by_item.field")),
            };

            let order = match map.get(&"order".to_string()) {
                Some(Value::Str(s)) => match s.as_str() {
                    "asc" => true,
                    "desc" => false,
                    _ => return Err(QueryParseError::InvalidField("order", "asc or desc")),
                },
                Some(_) => return Err(QueryParseError::InvalidType("order", "string")),
                None => return Err(QueryParseError::MissingField("order")),
            };

            let nulls = match map.get(&"nulls".to_string()) {
                Some(Value::Str(s)) => match s.as_str() {
                    "first" => Some(true),
                    "last" => Some(false),
                    _ => None,
                },
                Some(_) => return Err(QueryParseError::InvalidType("nulls", "string")),
                None => None,
            };

            let mut item = if order {
                OrderByItem::asc(&field)
            } else {
                OrderByItem::desc(&field)
            };

            if let Some(nulls_first) = nulls {
                if nulls_first {
                    item = item.nulls_first();
                } else {
                    item = item.nulls_last();
                }
            }

            Ok(item)
        }
        _ => Err(QueryParseError::InvalidType("order_by_item", "object")),
    }
}

/// Parse Pagination from QueryValue.
///
/// Determines mode by keys present:
/// - `page` + `page_size` → `Pagination::Page`
/// - `limit` and/or `offset` → `Pagination::LimitOffset`
pub fn pagination_from_value(value: &QueryValue) -> Result<Pagination, QueryParseError> {
    match value {
        Value::Map(map) => {
            // Check for page-based pagination first
            if let Some(Value::Int(page)) = map.get(&"page".to_string()) {
                let page_size = match map.get(&"page_size".to_string()) {
                    Some(Value::Int(ps)) => *ps as u64,
                    _ => return Err(QueryParseError::MissingField("limit.page_size")),
                };
                return Ok(Pagination::Page {
                    page: *page as u64,
                    page_size,
                });
            }

            // Fall back to limit/offset
            let limit = match map.get(&"limit".to_string()) {
                Some(Value::Int(i)) => Some(*i as u64),
                _ => None,
            };

            let offset = match map.get(&"offset".to_string()) {
                Some(Value::Int(i)) => *i as u64,
                _ => 0,
            };

            Ok(Pagination::LimitOffset { limit, offset })
        }
        _ => Err(QueryParseError::InvalidType("limit", "object")),
    }
}

/// Backward-compatible alias
#[deprecated(note = "Use pagination_from_value instead")]
pub fn limit_offset_from_value(value: &QueryValue) -> Result<Pagination, QueryParseError> {
    pagination_from_value(value)
}

/// Parse AggFunc from string (used by SELECT-specific parsers)
pub fn agg_func_from_str(s: &str) -> Result<AggFunc, QueryParseError> {
    match s {
        "count" => Ok(AggFunc::Count),
        "sum" => Ok(AggFunc::Sum),
        "avg" => Ok(AggFunc::Avg),
        "min" => Ok(AggFunc::Min),
        "max" => Ok(AggFunc::Max),
        _ => Err(QueryParseError::UnknownAggregateFunction(s.to_string())),
    }
}

/// Parse AggregateField from QueryValue (used by SELECT-specific parsers)
pub fn aggregate_field_from_value(value: &QueryValue) -> Result<AggregateField, QueryParseError> {
    match value {
        Value::Map(map) => {
            let type_str = match map.get(&"type".to_string()) {
                Some(Value::Str(s)) => s.as_str(),
                _ => return Err(QueryParseError::MissingField("field.type")),
            };

            match type_str {
                "field" => {
                    let name = match map.get(&"name".to_string()) {
                        Some(Value::Str(s)) => s.clone(),
                        _ => return Err(QueryParseError::MissingField("field.name")),
                    };
                    Ok(AggregateField::Field(name))
                }
                "all" => Ok(AggregateField::All),
                _ => Err(QueryParseError::UnknownType(type_str.to_string())),
            }
        }
        Value::Str(s) => Ok(AggregateField::Field(s.clone())),
        _ => Err(QueryParseError::InvalidType(
            "aggregate.field",
            "object or string",
        )),
    }
}
