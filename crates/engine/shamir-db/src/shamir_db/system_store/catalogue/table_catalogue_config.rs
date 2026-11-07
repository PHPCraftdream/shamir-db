use crate::types::value::QueryValue;
use crate::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;

pub(crate) fn table_config_from_catalogue_record(
    name: &str,
    record: &QueryValue,
) -> DbResult<crate::engine::table::TableConfig> {
    let mut config = crate::engine::table::TableConfig::new(name);
    if record.get("enable_indexes").and_then(QueryValue::as_bool) == Some(true) {
        config = config.with_indexes();
    }
    if let Some(value) = record.get("store_id") {
        let raw = value
            .as_str()
            .ok_or_else(|| DbError::Internal("invalid table store_id type".into()))?;
        config = config.with_store_id(
            raw.parse::<RecordId>()
                .map_err(|error| DbError::Internal(format!("invalid table store_id: {error}")))?,
        );
    }
    for (key, field_name) in [
        ("rename_from", "rename_from"),
        ("rename_from_store_id", "rename_from_store_id"),
    ] {
        if let Some(value) = record.get(key) {
            let text = value
                .as_str()
                .ok_or_else(|| DbError::Internal(format!("invalid table {field_name} type")))?;
            if key == "rename_from_store_id" {
                text.parse::<RecordId>().map_err(|error| {
                    DbError::Internal(format!("invalid table rename_from_store_id: {error}"))
                })?;
            }
        }
    }
    Ok(config)
}
