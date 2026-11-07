use crate::engine::repo::RenamePersistOutcome;
use crate::types::value::QueryValue;
use crate::DbError;

use super::super::store::TABLE_TABLES;
use super::super::SystemStore;
use super::rename_key::TableRenameKey;

impl SystemStore {
    pub(crate) async fn resolve_table_rename(
        &self,
        key: TableRenameKey<'_>,
        source_store_id: Option<&shamir_types::types::record_id::RecordId>,
        destination_store_id: &shamir_types::types::record_id::RecordId,
    ) -> RenamePersistOutcome {
        let TableRenameKey {
            db_name,
            repo_name,
            from,
            to,
        } = key;
        let destination = match self.load_table_record(db_name, repo_name, to).await {
            Ok(Some(row)) => row,
            Ok(None) => {
                return RenamePersistOutcome::InDoubt(DbError::Storage(
                    "rename destination is not visible".into(),
                ))
            }
            Err(error) => return RenamePersistOutcome::InDoubt(error),
        };
        match self.load_table_record(db_name, repo_name, from).await {
            Ok(None) => (),
            Ok(Some(_)) => {
                return RenamePersistOutcome::InDoubt(DbError::Storage(
                    "rename source remains visible".into(),
                ))
            }
            Err(error) => return RenamePersistOutcome::InDoubt(error),
        }
        if !expected_destination(&destination, key, source_store_id, destination_store_id) {
            return RenamePersistOutcome::InDoubt(DbError::Storage(
                "rename destination provenance differs".into(),
            ));
        }
        let table = match self.table(TABLE_TABLES).await {
            Ok(table) => table,
            Err(error) => return RenamePersistOutcome::InDoubt(error),
        };
        match self.flush_system_repo_durable(&table).await {
            Ok(()) => RenamePersistOutcome::Committed,
            Err(error) => RenamePersistOutcome::InDoubt(error),
        }
    }
}

fn expected_destination(
    row: &QueryValue,
    key: TableRenameKey<'_>,
    source_id: Option<&shamir_types::types::record_id::RecordId>,
    destination_id: &shamir_types::types::record_id::RecordId,
) -> bool {
    let TableRenameKey {
        db_name,
        repo_name,
        from,
        to,
    } = key;
    let source_id = source_id.map(ToString::to_string);
    let destination_id = destination_id.to_string();
    row.get("db_name").and_then(QueryValue::as_str) == Some(db_name)
        && row.get("repo_name").and_then(QueryValue::as_str) == Some(repo_name)
        && row.get("table_name").and_then(QueryValue::as_str) == Some(to)
        && row.get("store_id").and_then(QueryValue::as_str) == Some(destination_id.as_str())
        && row.get("rename_from").and_then(QueryValue::as_str) == Some(from)
        && match source_id.as_deref() {
            Some(source_id) => {
                row.get("rename_from_store_id").and_then(QueryValue::as_str) == Some(source_id)
            }
            None => row.get("rename_from_store_id").is_none(),
        }
}
