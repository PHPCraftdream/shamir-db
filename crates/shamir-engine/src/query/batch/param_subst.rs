use shamir_types::types::common::TMap;
use shamir_types::types::value::{InnerValue, QueryValue, Value};

/// Convert a scalar `InnerValue` to `QueryValue`.
///
/// Only the variants that can arrive in a `$param` binding are handled —
/// params are resolved from `FilterValue` via `resolve_filter_value`, which
/// produces only Null/Bool/Int/F64/Str. Map/List/Set/Bin etc. never appear
/// here; we return `QueryValue::Null` for unrepresentable variants
/// (safe: no data loss since those variants cannot be bound as params).
pub(super) fn scalar_inner_to_query_value(v: &InnerValue) -> QueryValue {
    match v {
        InnerValue::Null => QueryValue::Null,
        InnerValue::Bool(b) => QueryValue::Bool(*b),
        InnerValue::Int(i) => QueryValue::Int(*i),
        InnerValue::F64(f) => QueryValue::F64(*f),
        InnerValue::Str(s) => QueryValue::Str(s.clone()),
        // Binary / List / Set / Map cannot appear in param bindings (see above).
        _ => QueryValue::Null,
    }
}

/// Return `true` if `value` contains any `{ "$param": "..." }` node at any depth.
pub(super) fn contains_param_ref(value: &QueryValue) -> bool {
    match value {
        Value::Map(map) => {
            if map.len() == 1 {
                if let Some(Value::Str(_)) = map.get("$param") {
                    return true;
                }
            }
            map.values().any(contains_param_ref)
        }
        Value::List(arr) => arr.iter().any(contains_param_ref),
        _ => false,
    }
}

/// Recursively substitute `{ "$param": "<name>" }` objects inside a
/// `QueryValue` with the corresponding resolved `InnerValue`.
///
/// An object is a `$param` reference if and only if it has exactly one key
/// named `"$param"` whose value is a string.
///
/// Returns `Ok(owned value)` with substitutions applied.
///
/// Errors with the unresolvable param name (a `String`) if a referenced
/// param is absent from `params`; the caller maps this to
/// `BatchError::query_coded(alias, "unbound_param", ...)`.
///
/// **Fast path**: pre-scan the tree for any `$param` node. If none exist,
/// return a clone immediately — no per-node allocation for the common case
/// where write values are plain records with no param references.
/// If a `$param` is found but `params` is empty (top-level or empty bind),
/// the substitution will error with `unbound_param`.
pub(super) fn substitute_params(
    value: &QueryValue,
    params: &TMap<String, InnerValue>,
) -> Result<QueryValue, String> {
    // Fast path: if the tree has no $param nodes, return unchanged.
    if !contains_param_ref(value) {
        return Ok(value.clone());
    }
    substitute_params_inner(value, params)
}

pub(super) fn substitute_params_inner(
    value: &QueryValue,
    params: &TMap<String, InnerValue>,
) -> Result<QueryValue, String> {
    match value {
        Value::Map(map) => {
            // Check if this object is exactly `{ "$param": "<name>" }`.
            if map.len() == 1 {
                if let Some(Value::Str(name)) = map.get("$param") {
                    return match params.get(name.as_str()) {
                        Some(inner) => Ok(scalar_inner_to_query_value(inner)),
                        None => Err(name.clone()),
                    };
                }
            }
            // Recurse into all fields.
            let mut new_map = shamir_types::types::common::new_map();
            for (k, v) in map {
                new_map.insert(k.clone(), substitute_params_inner(v, params)?);
            }
            Ok(Value::Map(new_map))
        }
        Value::List(arr) => {
            let mut new_arr = Vec::with_capacity(arr.len());
            for v in arr {
                new_arr.push(substitute_params_inner(v, params)?);
            }
            Ok(Value::List(new_arr))
        }
        // Scalars: nothing to substitute.
        other => Ok(other.clone()),
    }
}
