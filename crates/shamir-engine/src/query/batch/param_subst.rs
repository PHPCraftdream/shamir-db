use shamir_types::types::common::TMap;
use shamir_types::types::value::InnerValue;

/// Convert a scalar `InnerValue` to `serde_json::Value`.
///
/// Only the variants that can arrive in a `$param` binding are handled —
/// params are resolved from `FilterValue` via `resolve_filter_value`, which
/// produces only Null/Bool/Int/F64/Str. Map/List/Set/Bin etc. never appear
/// here; we return `serde_json::Value::Null` for unrepresentable variants
/// (safe: no data loss since those variants cannot be bound as params).
pub(super) fn scalar_inner_to_json(v: &InnerValue) -> serde_json::Value {
    match v {
        InnerValue::Null => serde_json::Value::Null,
        InnerValue::Bool(b) => serde_json::Value::Bool(*b),
        InnerValue::Int(i) => serde_json::Value::Number((*i).into()),
        InnerValue::F64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        InnerValue::Str(s) => serde_json::Value::String(s.clone()),
        // Binary / List / Set / Map cannot appear in param bindings (see above).
        _ => serde_json::Value::Null,
    }
}

/// Return `true` if `value` contains any `{ "$param": "..." }` node at any depth.
pub(super) fn contains_param_ref(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if map.len() == 1 {
                if let Some(serde_json::Value::String(_)) = map.get("$param") {
                    return true;
                }
            }
            map.values().any(contains_param_ref)
        }
        serde_json::Value::Array(arr) => arr.iter().any(contains_param_ref),
        _ => false,
    }
}

/// Recursively substitute `{ "$param": "<name>" }` objects inside a
/// `serde_json::Value` with the corresponding resolved `InnerValue`.
///
/// An object is a `$param` reference if and only if it has exactly one key
/// named `"$param"` whose value is a JSON string.
///
/// Returns `Ok(owned value)` with substitutions applied.
///
/// Errors with the unresolvable param name (a `String`) if a referenced
/// param is absent from `params`; the caller maps this to
/// `BatchError::query_coded(alias, "unbound_param", ...)`.
///
/// **Fast path**: pre-scan the tree for any `$param` node. If none exist,
/// return a clone immediately — no per-node allocation for the common case
/// where write values are plain JSON with no param references.
/// If a `$param` is found but `params` is empty (top-level or empty bind),
/// the substitution will error with `unbound_param`.
pub(super) fn substitute_params(
    value: &serde_json::Value,
    params: &TMap<String, InnerValue>,
) -> Result<serde_json::Value, String> {
    // Fast path: if the tree has no $param nodes, return unchanged.
    if !contains_param_ref(value) {
        return Ok(value.clone());
    }
    substitute_params_inner(value, params)
}

pub(super) fn substitute_params_inner(
    value: &serde_json::Value,
    params: &TMap<String, InnerValue>,
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::Object(map) => {
            // Check if this object is exactly `{ "$param": "<name>" }`.
            if map.len() == 1 {
                if let Some(serde_json::Value::String(name)) = map.get("$param") {
                    return match params.get(name.as_str()) {
                        Some(inner) => Ok(scalar_inner_to_json(inner)),
                        None => Err(name.clone()),
                    };
                }
            }
            // Recurse into all fields.
            let mut new_map = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                new_map.insert(k.clone(), substitute_params_inner(v, params)?);
            }
            Ok(serde_json::Value::Object(new_map))
        }
        serde_json::Value::Array(arr) => {
            let mut new_arr = Vec::with_capacity(arr.len());
            for v in arr {
                new_arr.push(substitute_params_inner(v, params)?);
            }
            Ok(serde_json::Value::Array(new_arr))
        }
        // Scalars: nothing to substitute.
        other => Ok(other.clone()),
    }
}
