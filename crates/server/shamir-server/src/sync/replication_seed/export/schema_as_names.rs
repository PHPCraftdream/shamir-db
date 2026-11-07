use shamir_types::core::interner::{Interner, InternerKey};
use shamir_types::types::value::QueryValue;

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

/// Schema metadata, not a constructed query. Numeric path values need semantic
/// conversion independently of record-map key translation.
pub(crate) fn schema_as_names(
    schema: &QueryValue,
    interner: &Interner,
) -> Result<QueryValue, SeedError> {
    if matches!(schema, QueryValue::Null) {
        return Ok(QueryValue::Null);
    }
    let QueryValue::List(rules) = schema else {
        return Err(invalid("schema must be a list"));
    };
    let mut result = Vec::with_capacity(rules.len());
    for rule in rules {
        let QueryValue::Map(fields) = rule else {
            return Err(invalid("schema rule must be a map"));
        };
        let mut fields = fields.clone();
        // Dependencies are not portable in R2-lite. Fail rather than omit.
        for key in [
            "foreign_key",
            "references",
            "default",
            "transform",
            "scalar",
            "auto_now",
            "auto_now_add",
        ] {
            if fields.contains_key(key) {
                return Err(invalid(format!("unsupported schema dependency: {key}")));
            }
        }
        let path = fields
            .get("path")
            .and_then(QueryValue::as_array)
            .ok_or_else(|| invalid("schema path missing"))?;
        if path.is_empty() {
            return Err(invalid("empty schema path"));
        }
        let mut names = Vec::with_capacity(path.len());
        for id in path {
            let id = id
                .as_i64()
                .and_then(|id| u64::try_from(id).ok())
                .ok_or_else(|| invalid("invalid schema path id"))?;
            let name = interner
                .get_str(&InternerKey::new(id))
                .ok_or_else(|| invalid("missing schema path binding"))?;
            if name.is_empty() {
                return Err(invalid("empty schema path binding"));
            }
            names.push(QueryValue::Str(name.to_string()));
        }
        fields.insert("path".into(), QueryValue::List(names));
        fields.sort_keys();
        result.push(QueryValue::Map(fields));
    }
    Ok(QueryValue::List(result))
}
