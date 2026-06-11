use shamir_query_types::filter::{Filter, FilterValue};

pub fn filter_matches_value(filter: &Filter, value: &serde_json::Value) -> bool {
    match filter {
        Filter::Eq { field, value: fv } => resolve_field(value, field) == filter_value_to_json(fv),
        Filter::Ne { field, value: fv } => resolve_field(value, field) != filter_value_to_json(fv),
        Filter::Gt { field, value: fv } => {
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv))
                == Some(std::cmp::Ordering::Greater)
        }
        Filter::Gte { field, value: fv } => matches!(
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv)),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        Filter::Lt { field, value: fv } => {
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv))
                == Some(std::cmp::Ordering::Less)
        }
        Filter::Lte { field, value: fv } => matches!(
            cmp_json(&resolve_field(value, field), &filter_value_to_json(fv)),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        Filter::In { field, values } => {
            let resolved = resolve_field(value, field);
            values.iter().any(|v| resolved == filter_value_to_json(v))
        }
        Filter::NotIn { field, values } => {
            let resolved = resolve_field(value, field);
            !values.iter().any(|v| resolved == filter_value_to_json(v))
        }
        Filter::IsNull { field } => resolve_field(value, field).is_null(),
        Filter::IsNotNull { field } => !resolve_field(value, field).is_null(),
        Filter::Exists { field } => !matches!(resolve_field(value, field), serde_json::Value::Null),
        Filter::NotExists { field } => {
            matches!(resolve_field(value, field), serde_json::Value::Null)
        }
        Filter::And { filters } => filters.iter().all(|f| filter_matches_value(f, value)),
        Filter::Or { filters } => filters.iter().any(|f| filter_matches_value(f, value)),
        Filter::Not { filter: f } => !filter_matches_value(f, value),
        // Unsupported variants should be rejected at grant time; if one
        // slips through, fail-closed (do not deliver).
        _ => false,
    }
}

fn resolve_field(value: &serde_json::Value, path: &[String]) -> serde_json::Value {
    let mut current = value;
    for segment in path {
        match current.get(segment.as_str()) {
            Some(v) => current = v,
            None => return serde_json::Value::Null,
        }
    }
    current.clone()
}

fn filter_value_to_json(fv: &FilterValue) -> serde_json::Value {
    match fv {
        FilterValue::Null => serde_json::Value::Null,
        FilterValue::Bool(b) => serde_json::Value::Bool(*b),
        FilterValue::Int(i) => serde_json::json!(*i),
        FilterValue::Float(f) => serde_json::json!(*f),
        FilterValue::String(s) => serde_json::Value::String(s.clone()),
        FilterValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(filter_value_to_json).collect())
        }
        _ => serde_json::Value::Null,
    }
}

fn cmp_json(a: &serde_json::Value, b: &serde_json::Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            a.as_f64().partial_cmp(&b.as_f64())
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}
