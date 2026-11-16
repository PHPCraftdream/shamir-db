use crate::admin::types::schema_migration::model::SchemaMigrationError;
use shamir_types::types::value::QueryValue;
#[cfg(test)]
pub(crate) fn canonical_value(value: &QueryValue) -> Result<Vec<u8>, SchemaMigrationError> {
    super::ddl::canonical_query_value(value).map_err(|_| {
        SchemaMigrationError::UnsafeOperation("query value cannot be canonically encoded")
    })
}
