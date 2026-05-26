//! Parse SELECT Query from QueryValue (JSON → Value<Map> → Query)
//!
//! This module converts QueryValue (Value<Map<String, Value>>) to typed SELECT Query struct.

use crate::query::common::{
    agg_func_from_str, aggregate_field_from_value, filter_from_value, group_by_from_value,
    order_by_from_value, pagination_from_value, QueryParseError,
};
use crate::query::read::{Pagination, ReadQuery, Select, SelectItem};
use crate::query::TableRef;
use shamir_types::types::value::{QueryValue, Value};

/// Parse SELECT Query from QueryValue (Value<Map<String, Value>>)
pub fn query_from_value(value: &QueryValue) -> Result<ReadQuery, QueryParseError> {
    let map = match value {
        Value::Map(m) => m,
        _ => return Err(QueryParseError::NotAnObject),
    };

    let from = match map.get("from") {
        Some(Value::Str(s)) => TableRef::new(s.clone()),
        Some(Value::List(parts)) if parts.len() == 2 => {
            if let (Some(Value::Str(repo)), Some(Value::Str(table))) = (parts.first(), parts.get(1))
            {
                TableRef::with_repo(repo.clone(), table.clone())
            } else {
                return Err(QueryParseError::InvalidType(
                    "from",
                    "string or [repo, table]",
                ));
            }
        }
        _ => return Err(QueryParseError::MissingField("from")),
    };

    let select = match map.get("select") {
        Some(v) => select_from_value(v)?,
        None => Select::all(),
    };

    let r#where = match map.get("where") {
        Some(v) => Some(filter_from_value(v)?),
        None => None,
    };

    let group_by = match map.get("group_by") {
        Some(v) => Some(group_by_from_value(v)?),
        None => None,
    };

    let order_by = match map.get("order_by") {
        Some(v) => Some(order_by_from_value(v)?),
        None => None,
    };

    let pagination = match map.get("limit") {
        Some(v) => pagination_from_value(v)?,
        None => Pagination::None,
    };

    let count_total = match map.get("count_total") {
        Some(Value::Bool(b)) => *b,
        _ => false,
    };

    Ok(ReadQuery {
        from,
        select,
        r#where,
        group_by,
        order_by,
        pagination,
        count_total,
    })
}

/// Parse Select from QueryValue
fn select_from_value(value: &QueryValue) -> Result<Select, QueryParseError> {
    match value {
        Value::Map(map) => {
            let items = match map.get("items") {
                Some(v) => items_from_value(v)?,
                None => Vec::new(),
            };

            let distinct = match map.get("distinct") {
                Some(Value::Bool(b)) => *b,
                _ => false,
            };

            Ok(Select { items, distinct })
        }
        _ => Err(QueryParseError::InvalidType("select", "object")),
    }
}

/// Parse SelectItem list from QueryValue
fn items_from_value(value: &QueryValue) -> Result<Vec<SelectItem>, QueryParseError> {
    match value {
        Value::List(items) => items.iter().map(item_from_value).collect(),
        _ => Err(QueryParseError::InvalidType("items", "array")),
    }
}

/// Parse single SelectItem from QueryValue
fn item_from_value(value: &QueryValue) -> Result<SelectItem, QueryParseError> {
    match value {
        Value::Map(map) => {
            let type_str = match map.get("type") {
                Some(Value::Str(s)) => s.as_str(),
                _ => return Err(QueryParseError::MissingField("item.type")),
            };

            match type_str {
                "all" => Ok(SelectItem::All),
                "field" => {
                    let path = match map.get("path") {
                        Some(Value::Str(s)) => vec![s.clone()],
                        Some(Value::List(list)) => list
                            .iter()
                            .filter_map(|v| match v {
                                Value::Str(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect(),
                        _ => return Err(QueryParseError::MissingField("field.path")),
                    };

                    let alias = match map.get("alias") {
                        Some(Value::Str(s)) => Some(s.clone()),
                        _ => None,
                    };

                    Ok(SelectItem::Field { path, alias })
                }
                "aggregate" => {
                    let func_str = match map.get("func") {
                        Some(Value::Str(s)) => s.as_str(),
                        _ => return Err(QueryParseError::MissingField("aggregate.func")),
                    };

                    let func = agg_func_from_str(func_str)?;

                    let field = match map.get("field") {
                        Some(v) => aggregate_field_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("aggregate.field")),
                    };

                    let alias = match map.get("alias") {
                        Some(Value::Str(s)) => Some(s.clone()),
                        _ => None,
                    };

                    let distinct = match map.get("distinct") {
                        Some(Value::Bool(b)) => *b,
                        _ => false,
                    };

                    Ok(SelectItem::Aggregate {
                        func,
                        field,
                        alias,
                        distinct,
                    })
                }
                "count_all" => {
                    let alias = match map.get("alias") {
                        Some(Value::Str(s)) => Some(s.clone()),
                        _ => None,
                    };

                    Ok(SelectItem::CountAll { alias })
                }
                "expr" => {
                    use crate::query::common::expr_from_value;

                    let expr = match map.get("expr") {
                        Some(v) => expr_from_value(v)?,
                        None => return Err(QueryParseError::MissingField("expr.expr")),
                    };

                    let alias = match map.get("alias") {
                        Some(Value::Str(s)) => Some(s.clone()),
                        _ => None,
                    };

                    Ok(SelectItem::Expression { expr, alias })
                }
                _ => Err(QueryParseError::UnknownType(type_str.to_string())),
            }
        }
        _ => Err(QueryParseError::InvalidType("item", "object")),
    }
}
