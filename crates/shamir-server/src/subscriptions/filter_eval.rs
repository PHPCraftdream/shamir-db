use std::borrow::Cow;

use shamir_query_types::filter::{Filter, FilterValue};

pub fn filter_matches_value(filter: &Filter, value: &serde_json::Value) -> bool {
    match filter {
        Filter::Eq { field, value: fv } => json_eq_fv(resolve_field(value, field), fv),
        Filter::Ne { field, value: fv } => !json_eq_fv(resolve_field(value, field), fv),
        Filter::Gt { field, value: fv } => {
            cmp_json_fv(resolve_field(value, field), fv) == Some(std::cmp::Ordering::Greater)
        }
        Filter::Gte { field, value: fv } => matches!(
            cmp_json_fv(resolve_field(value, field), fv),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        Filter::Lt { field, value: fv } => {
            cmp_json_fv(resolve_field(value, field), fv) == Some(std::cmp::Ordering::Less)
        }
        Filter::Lte { field, value: fv } => matches!(
            cmp_json_fv(resolve_field(value, field), fv),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        Filter::In { field, values } => {
            let resolved = resolve_field(value, field);
            values.iter().any(|v| json_eq_fv(resolved, v))
        }
        Filter::NotIn { field, values } => {
            let resolved = resolve_field(value, field);
            !values.iter().any(|v| json_eq_fv(resolved, v))
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

/// Walk a field path and return a reference into the original JSON value.
/// Returns `&serde_json::Value::Null` (via a static) when any segment is
/// missing, avoiding a heap allocation for the miss case.
#[inline]
fn resolve_field<'v>(value: &'v serde_json::Value, path: &[String]) -> &'v serde_json::Value {
    let mut current = value;
    for segment in path {
        match current.get(segment.as_str()) {
            Some(v) => current = v,
            None => return &serde_json::Value::Null,
        }
    }
    current
}

/// Compare a resolved JSON value against a `FilterValue` without allocating.
/// Returns `true` when the two represent equal values.
#[inline]
fn json_eq_fv(json: &serde_json::Value, fv: &FilterValue) -> bool {
    match fv {
        FilterValue::Null => json.is_null(),
        FilterValue::Bool(b) => json.as_bool() == Some(*b),
        FilterValue::Int(i) => {
            // serde_json stores integers; compare via i64 when available, else
            // fall back to f64 to handle numbers that arrived as floats.
            if let Some(v) = json.as_i64() {
                v == *i
            } else {
                json.as_f64() == Some(*i as f64)
            }
        }
        FilterValue::Float(f) => json.as_f64() == Some(*f),
        FilterValue::String(s) => json.as_str() == Some(s.as_str()),
        FilterValue::Array(arr) => {
            // For array equality we must compare element-by-element, which
            // may need allocation for nested arrays; keep allocation-free for
            // the common flat case via recursion.
            match json {
                serde_json::Value::Array(json_arr) => {
                    json_arr.len() == arr.len()
                        && json_arr
                            .iter()
                            .zip(arr.iter())
                            .all(|(j, f)| json_eq_fv(j, f))
                }
                _ => false,
            }
        }
        // Non-primitive FilterValue variants (FieldRef, QueryRef, FnCall,
        // Expr, Cond, Param) are not evaluated here — they require engine
        // context that is not available at subscription eval time.
        _ => false,
    }
}

/// Order a resolved JSON value against a `FilterValue` without allocating.
fn cmp_json_fv(json: &serde_json::Value, fv: &FilterValue) -> Option<std::cmp::Ordering> {
    match fv {
        FilterValue::Int(i) => {
            let b = *i as f64;
            json.as_f64()?.partial_cmp(&b)
        }
        FilterValue::Float(f) => json.as_f64()?.partial_cmp(f),
        FilterValue::String(s) => {
            let a = json.as_str()?;
            Some(a.cmp(s.as_str()))
        }
        _ => None,
    }
}

/// Convert a `FilterValue` to a `serde_json::Value`, borrowing where the
/// variant already holds a `serde_json`-compatible owned value, allocating
/// only when necessary.  Used by callers that genuinely need a `Value` (e.g.
/// for display / serialisation); hot comparison paths use `json_eq_fv` /
/// `cmp_json_fv` directly.
#[allow(dead_code)]
fn filter_value_to_json(fv: &FilterValue) -> Cow<'_, serde_json::Value> {
    match fv {
        FilterValue::Null => Cow::Owned(serde_json::Value::Null),
        FilterValue::Bool(b) => Cow::Owned(serde_json::Value::Bool(*b)),
        FilterValue::Int(i) => Cow::Owned(serde_json::json!(*i)),
        FilterValue::Float(f) => Cow::Owned(serde_json::json!(*f)),
        FilterValue::String(s) => Cow::Owned(serde_json::Value::String(s.clone())),
        FilterValue::Array(arr) => Cow::Owned(serde_json::Value::Array(
            arr.iter()
                .map(|v| filter_value_to_json(v).into_owned())
                .collect(),
        )),
        _ => Cow::Owned(serde_json::Value::Null),
    }
}
