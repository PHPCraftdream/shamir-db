use std::collections::BTreeMap;

use shamir_engine::db_instance::replay_to::ReplayScope;
use shamir_types::types::record_id::RecordId;

use crate::types::value::QueryValue;
use crate::{DbError, DbResult};

use super::super::SystemStore;

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

/// Derive the [`ReplayScope`] of every captured `(db, repo)` key from the
/// system catalogue, exactly the way normal boot does: one `load_tables`
/// pass over the catalogue, decoded by the same
/// `table_config_from_catalogue_record` boot uses (physical store ids,
/// index flag, rename metadata validation). No user store is opened.
///
/// `captured` maps each `(db, repo)` to the manifest's canonical
/// forward-slash repository path. Before any scope is derived, the raw
/// repository catalogue is checked: a captured repository whose raw
/// `path` does not exactly equal the expected path would not relocate to
/// the staged directory on the later swap, so the mismatch fails closed
/// and names the repository. A captured repository with no catalogue row
/// at all is the design's Q9 dropped-after-capture orphan and is
/// tolerated (empty scope).
///
/// Captured keys with zero catalogue table rows map to an empty scope.
/// Ordinary rows of non-captured repositories are ignored. A corrupted
/// row fails closed and names the owning repository: a table row without
/// a table name, or without its `db_name` / `repo_name` route (the
/// catalogue schema has no known rows without them); decoder failures
/// carry the owning `(db, repo)` context.
pub async fn load_replay_scopes(
    store: &SystemStore,
    captured: &BTreeMap<(&str, &str), &str>,
) -> DbResult<BTreeMap<(String, String), ReplayScope>> {
    // Preallocate every captured key (borrowed, no per-row allocation) so
    // repos without table rows still come back with an (empty) scope.
    let mut configs_by_repo: BTreeMap<(&str, &str), Vec<crate::engine::table::TableConfig>> =
        captured
            .keys()
            .copied()
            .map(|key| (key, Vec::new()))
            .collect();
    // Relocation gate: the raw (catalogue-form) repository path of every
    // captured repo must equal the manifest's expected path, or the later
    // swap would not relocate the staged copy.
    for record in store.load_raw_repositories().await? {
        let db = record["db_name"].as_str().ok_or_else(|| {
            DbError::Internal(
                "corrupt repository catalogue: repository row is missing db_name".to_string(),
            )
        })?;
        let repo = record["repo_name"].as_str().ok_or_else(|| {
            DbError::Internal(
                "corrupt repository catalogue: repository row is missing repo_name".to_string(),
            )
        })?;
        if let Some(expected_path) = captured.get(&(db, repo)) {
            let actual_path = record["path"].as_str().ok_or_else(|| {
                DbError::Internal(format!(
                    "repository ('{db}', '{repo}') is captured but its catalogue row has no path"
                ))
            })?;
            if actual_path != *expected_path {
                return Err(DbError::Internal(format!(
                    "repository ('{db}', '{repo}') catalogue path {actual_path:?} does not match the captured path {expected_path:?}"
                )));
            }
        }
    }
    let records = store.load_tables().await?;
    for record in &records {
        let db = record["db_name"].as_str().ok_or_else(|| {
            DbError::Internal("corrupt table catalogue: table row is missing db_name".to_string())
        })?;
        let repo = record["repo_name"].as_str().ok_or_else(|| {
            DbError::Internal("corrupt table catalogue: table row is missing repo_name".to_string())
        })?;
        let Some(configs) = configs_by_repo.get_mut(&(db, repo)) else {
            continue;
        };
        let Some(name) = record["table_name"].as_str() else {
            return Err(DbError::Internal(format!(
                "corrupt table catalogue: repository ('{db}', '{repo}') has a table row without a table name"
            )));
        };
        configs.push(
            table_config_from_catalogue_record(name, record).map_err(|error| {
                DbError::Internal(format!(
                    "table '{name}' of repository ('{db}', '{repo}'): {error}"
                ))
            })?,
        );
    }
    Ok(configs_by_repo
        .into_iter()
        .map(|((db, repo), configs)| {
            (
                (db.to_string(), repo.to_string()),
                ReplayScope::new(configs),
            )
        })
        .collect())
}
