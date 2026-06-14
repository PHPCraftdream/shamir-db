use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::value::{InnerValue, QueryValue, Value};

/// Convert an [`InnerValue`] (interned keys) to a [`QueryValue`] (string
/// keys) using the given interner. Used by `run_validators` to build the
/// `record` / `old_record` params that a validator receives.
///
/// This is a lightweight recursive conversion that avoids the JSON
/// round-trip of `inner_to_json_value` + `json_value_to_inner`.
pub fn inner_to_query_value(value: &InnerValue, interner: &Interner) -> Result<QueryValue, String> {
    inner_to_query_value_with(value, &|key| interner.with_str(key, |s| s.to_string()))
}

/// Like [`inner_to_query_value`] but resolves interned keys through an
/// arbitrary `resolve` closure (`InternerKey → Option<String>`).
///
/// The tx write path interns brand-new field names into a per-tx LAYERED
/// interner overlay (ids ≥ `OVERLAY_ID_BASE`) that the GLOBAL interner
/// cannot yet resolve. Validators run BEFORE commit (before the overlay is
/// merged into base), so converting a just-staged record with the base
/// interner alone fails with `interned key … not found`. The tx execute
/// paths pass a resolver that consults the overlay first, so a new field's
/// key resolves at validation time.
pub fn inner_to_query_value_with(
    value: &InnerValue,
    resolve: &dyn Fn(&InternerKey) -> Option<String>,
) -> Result<QueryValue, String> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(b) => Ok(Value::Bool(*b)),
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::F64(f) => Ok(Value::F64(*f)),
        Value::Dec(d) => Ok(Value::Dec(*d)),
        Value::Big(b) => Ok(Value::Big(b.clone())),
        Value::Str(s) => Ok(Value::Str(s.clone())),
        Value::Bin(b) => Ok(Value::Bin(b.clone())),
        Value::List(l) => {
            let mut out = Vec::with_capacity(l.len());
            for v in l {
                out.push(inner_to_query_value_with(v, resolve)?);
            }
            Ok(Value::List(out))
        }
        Value::Set(s) => {
            let mut out = shamir_types::types::common::new_set();
            for v in s {
                out.insert(inner_to_query_value_with(v, resolve)?);
            }
            Ok(Value::Set(out))
        }
        Value::Map(m) => {
            let mut out = shamir_types::types::common::new_map();
            for (k, v) in m {
                let key_str =
                    resolve(k).ok_or_else(|| format!("interned key {:?} not found", k))?;
                out.insert(key_str, inner_to_query_value_with(v, resolve)?);
            }
            Ok(Value::Map(out))
        }
    }
}
