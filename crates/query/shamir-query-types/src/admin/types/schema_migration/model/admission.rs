use crate::admin::types::schema_ops::{FieldRuleDto, NumDto};
use crate::filter::FilterValue;
use shamir_types::types::value::QueryValue;

use super::SchemaMigrationError;

const MAX_VALUE_DEPTH: usize = 64;
const MAX_VALUE_NODES: usize = 65_536;
const MAX_VALUE_BYTES: usize = 16 * 1024 * 1024;

pub fn validate_schema_rules(rules: &[FieldRuleDto]) -> Result<(), SchemaMigrationError> {
    for rule in rules {
        if rule.constraints.scalar.is_some() {
            return Err(SchemaMigrationError::UnsafeOperation(
                "registered scalar constraints are not admitted in schema migrations",
            ));
        }
        if rule.constraints.compare.is_some() {
            return Err(SchemaMigrationError::UnsafeOperation(
                "cross-field schema comparisons are dynamic and not admitted",
            ));
        }
        let mut default_bytes = 0;
        let mut default_nodes = 0;
        validate_filter_value(
            rule.constraints.default.as_ref(),
            0,
            &mut default_bytes,
            &mut default_nodes,
        )?;
        if let Some(values) = &rule.constraints.one_of {
            validate_query_values(values)?;
        }
        validate_num(rule.constraints.min.as_ref())?;
        validate_num(rule.constraints.max.as_ref())?;
    }
    Ok(())
}

fn validate_num(value: Option<&NumDto>) -> Result<(), SchemaMigrationError> {
    if matches!(value, Some(NumDto::F64(number)) if !number.is_finite()) {
        return Err(SchemaMigrationError::UnsafeOperation(
            "non-finite numeric schema constraint is not canonical",
        ));
    }
    Ok(())
}

fn validate_filter_value(
    value: Option<&FilterValue>,
    depth: usize,
    bytes: &mut usize,
    nodes: &mut usize,
) -> Result<(), SchemaMigrationError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(SchemaMigrationError::UnsafeOperation(
            "schema default nesting exceeds canonical bound",
        ));
    }
    let Some(value) = value else {
        return Ok(());
    };
    count_filter_node(nodes)?;
    match value {
        FilterValue::Null => Err(SchemaMigrationError::UnsafeOperation(
            "an explicit null default is indistinguishable from an absent default on the wire",
        )),
        FilterValue::Bool(_) | FilterValue::Int(_) => Ok(()),
        FilterValue::String(value) => check_value_bytes(bytes, value.len()),
        FilterValue::Binary(value) => check_value_bytes(bytes, value.len()),
        FilterValue::Float(number) if number.is_finite() => Ok(()),
        FilterValue::Float(_) => Err(SchemaMigrationError::UnsafeOperation(
            "non-finite schema default is not canonical",
        )),
        FilterValue::Array(values) => {
            for value in values {
                validate_filter_value(Some(value), depth + 1, bytes, nodes)?;
            }
            Ok(())
        }
        FilterValue::FieldRef { .. }
        | FilterValue::QueryRef { .. }
        | FilterValue::FnCall { .. }
        | FilterValue::Expr { .. }
        | FilterValue::Cond { .. }
        | FilterValue::Param { .. } => Err(SchemaMigrationError::UnsafeOperation(
            "schema defaults must be literal values; dynamic expressions are not admitted",
        )),
    }
}

pub fn validate_query_values(values: &[QueryValue]) -> Result<(), SchemaMigrationError> {
    let mut nodes = 0_usize;
    let mut bytes = 0_usize;
    for value in values {
        validate_query_value_inner(value, 0, &mut nodes, &mut bytes)?;
    }
    Ok(())
}

pub fn validate_query_value(value: &QueryValue, depth: usize) -> Result<(), SchemaMigrationError> {
    let mut nodes = 0_usize;
    let mut bytes = 0_usize;
    validate_query_value_inner(value, depth, &mut nodes, &mut bytes)
}

fn count_filter_node(nodes: &mut usize) -> Result<(), SchemaMigrationError> {
    *nodes = nodes
        .checked_add(1)
        .ok_or(SchemaMigrationError::UnsafeOperation(
            "schema default node count overflow",
        ))?;
    if *nodes > MAX_VALUE_NODES {
        return Err(SchemaMigrationError::UnsafeOperation(
            "schema default exceeds canonical node bound",
        ));
    }
    Ok(())
}

fn check_value_bytes(total: &mut usize, size: usize) -> Result<(), SchemaMigrationError> {
    *total = total
        .checked_add(size)
        .ok_or(SchemaMigrationError::UnsafeOperation(
            "query-value byte count overflow",
        ))?;
    if *total > MAX_VALUE_BYTES {
        return Err(SchemaMigrationError::UnsafeOperation(
            "literal values exceed the 16 MiB canonical size bound",
        ));
    }
    Ok(())
}

fn validate_query_value_inner(
    value: &QueryValue,
    depth: usize,
    nodes: &mut usize,
    bytes: &mut usize,
) -> Result<(), SchemaMigrationError> {
    *nodes = nodes
        .checked_add(1)
        .ok_or(SchemaMigrationError::UnsafeOperation(
            "query-value node count overflow",
        ))?;
    if *nodes > MAX_VALUE_NODES || depth > MAX_VALUE_DEPTH {
        return Err(SchemaMigrationError::UnsafeOperation(
            "query value exceeds canonical nesting or node bound",
        ));
    }
    match value {
        QueryValue::Null | QueryValue::Bool(_) | QueryValue::Int(_) => Ok(()),
        QueryValue::Big(_) | QueryValue::Dec(_) | QueryValue::Set(_) => {
            Err(SchemaMigrationError::UnsafeOperation(
                "big integers, decimals, and sets are not admitted because wire serialization does not preserve their value variants",
            ))
        }
        QueryValue::Str(value) => check_value_bytes(bytes, value.len()),
        QueryValue::Bin(value) => check_value_bytes(bytes, value.len()),
        QueryValue::F64(number) if number.is_finite() => Ok(()),
        QueryValue::F64(_) => Err(SchemaMigrationError::UnsafeOperation(
            "non-finite query-value float is not canonical",
        )),
        QueryValue::List(values) => {
            for nested in values {
                validate_query_value_inner(nested, depth + 1, nodes, bytes)?;
            }
            Ok(())
        }
        QueryValue::Map(values) => {
            for (key, nested) in values {
                check_value_bytes(bytes, key.len())?;
                validate_query_value_inner(nested, depth + 1, nodes, bytes)?;
            }
            Ok(())
        }
    }
}
