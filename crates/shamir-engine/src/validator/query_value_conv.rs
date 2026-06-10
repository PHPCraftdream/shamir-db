use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::value::{InnerValue, QueryValue, Value};

/// Convert an [`InnerValue`] (interned keys) to a [`QueryValue`] (string
/// keys) using the given interner. Used by `run_validators` to build the
/// `record` / `old_record` params that a validator receives.
///
/// This is a lightweight recursive conversion that avoids the JSON
/// round-trip of `inner_to_json_value` + `json_value_to_inner`.
pub fn inner_to_query_value(value: &InnerValue, interner: &Interner) -> Result<QueryValue, String> {
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
                out.push(inner_to_query_value(v, interner)?);
            }
            Ok(Value::List(out))
        }
        Value::Set(s) => {
            let mut out = shamir_types::types::common::new_set();
            for v in s {
                out.insert(inner_to_query_value(v, interner)?);
            }
            Ok(Value::Set(out))
        }
        Value::Map(m) => {
            let mut out = shamir_types::types::common::new_map();
            for (k, v) in m {
                let key_str = deintern(interner, k)?;
                out.insert(key_str, inner_to_query_value(v, interner)?);
            }
            Ok(Value::Map(out))
        }
    }
}

/// Helper: resolve an interned key to its string form.
fn deintern(interner: &Interner, key: &InternerKey) -> Result<String, String> {
    interner
        .with_str(key, |s| s.to_string())
        .ok_or_else(|| format!("interned key {:?} not found", key))
}
