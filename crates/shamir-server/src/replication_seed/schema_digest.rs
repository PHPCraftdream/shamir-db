use shamir_types::types::value::QueryValue;

use super::codec::{digest, encode};
use super::{SeedError, TableRecord};

/// Data counts and excluded index state do not participate in the DDL digest.
pub(crate) fn schema_digest(tables: &[TableRecord]) -> Result<String, SeedError> {
    let schemas: Vec<_> = tables
        .iter()
        .map(|table| {
            let mut schema = table.schema.clone();
            canonicalize(&mut schema);
            (&table.name, table.schema_version, schema)
        })
        .collect();
    Ok(digest(&encode(&schemas)?))
}

fn canonicalize(value: &mut QueryValue) {
    match value {
        QueryValue::Map(map) => {
            for value in map.values_mut() {
                canonicalize(value);
            }
            map.sort_keys();
        }
        QueryValue::List(list) => {
            for value in list {
                canonicalize(value);
            }
        }
        QueryValue::Set(_)
        | QueryValue::Null
        | QueryValue::Bool(_)
        | QueryValue::Int(_)
        | QueryValue::F64(_)
        | QueryValue::Dec(_)
        | QueryValue::Big(_)
        | QueryValue::Str(_)
        | QueryValue::Bin(_) => {}
    }
}
