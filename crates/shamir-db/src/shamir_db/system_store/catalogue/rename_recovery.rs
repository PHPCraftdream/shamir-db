use crate::access::Actor;
use crate::types::value::QueryValue;
use crate::{DbError, DbResult};

use super::super::store::TABLE_TABLES;
use super::super::SystemStore;
use super::rename_key::TableRenameKey;

impl SystemStore {
    pub(crate) async fn complete_table_rename_row(
        &self,
        key: TableRenameKey<'_>,
        source_store_id: Option<&shamir_types::types::record_id::RecordId>,
        source_row_present: bool,
        destination_store_id: &shamir_types::types::record_id::RecordId,
        expected_destination: &QueryValue,
    ) -> DbResult<()> {
        let TableRenameKey {
            db_name,
            repo_name,
            from,
            to,
        } = key;
        self.catalogue_write_fault_gate()?;
        let QueryValue::Map(fields) = expected_destination else {
            return Err(DbError::Internal(
                "rename destination row is not a map".into(),
            ));
        };
        let source_id_text = fields.get("rename_from_store_id");
        let source_id = match source_id_text {
            None => None,
            Some(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| DbError::Internal("invalid source store_id type".into()))?;
                Some(
                    text.parse::<shamir_types::types::record_id::RecordId>()
                        .map_err(|error| {
                            DbError::Internal(format!("invalid source store_id: {error}"))
                        })?,
                )
            }
        };
        let source_id_text = source_id.as_ref().map(ToString::to_string);
        if source_id.as_ref() != source_store_id {
            return Err(DbError::Validation(
                "rename source incarnation changed".into(),
            ));
        }
        let destination_id = destination_store_id.to_string();
        if fields.get("db_name").and_then(QueryValue::as_str) != Some(db_name)
            || fields.get("repo_name").and_then(QueryValue::as_str) != Some(repo_name)
            || fields.get("table_name").and_then(QueryValue::as_str) != Some(to)
            || fields.get("rename_from").and_then(QueryValue::as_str) != Some(from)
            || fields
                .get("rename_from_store_id")
                .and_then(QueryValue::as_str)
                != source_id_text.as_deref()
            || fields.get("store_id").and_then(QueryValue::as_str) != Some(destination_id.as_str())
        {
            return Err(DbError::Validation(
                "rename destination incarnation changed".into(),
            ));
        }
        let source_row = self.load_table_record(db_name, repo_name, from).await?;
        if source_row_present != source_row.is_some() {
            return Err(DbError::Validation(
                "rename source catalogue row changed".into(),
            ));
        }
        if let Some(source) = source_row {
            let QueryValue::Map(fields) = source else {
                return Err(DbError::Validation("rename source row is malformed".into()));
            };
            let source_store_id = match fields.get("store_id") {
                None => None,
                Some(value) => {
                    let text = value.as_str().ok_or_else(|| {
                        DbError::Validation("rename source row is malformed".into())
                    })?;
                    Some(
                        text.parse::<shamir_types::types::record_id::RecordId>()
                            .map_err(|_| {
                                DbError::Validation("rename source row is malformed".into())
                            })?,
                    )
                }
            };
            if source_store_id.as_ref() != source_id.as_ref() {
                return Err(DbError::Validation(
                    "rename source incarnation changed".into(),
                ));
            }
        }
        let current_destination = self.load_table_record(db_name, repo_name, to).await?;
        if current_destination.as_ref() != Some(expected_destination) {
            return Err(DbError::Validation(
                "rename destination catalogue row changed".into(),
            ));
        }
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: QueryValue::Map({
                let mut key = crate::types::common::new_map();
                key.insert("db_name".into(), QueryValue::Str(db_name.into()));
                key.insert("repo_name".into(), QueryValue::Str(repo_name.into()));
                key.insert("table_name".into(), QueryValue::Str(to.into()));
                key
            }),
            value: expected_destination.clone(),
        };
        let source_op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_TABLES),
            where_clause: crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".into()],
                        value: crate::query::filter::FilterValue::String(db_name.into()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".into()],
                        value: crate::query::filter::FilterValue::String(repo_name.into()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["table_name".into()],
                        value: crate::query::filter::FilterValue::String(from.into()),
                    },
                ],
            },
            select: None,
            expected_version: None,
        };
        self.persist_table_interner(&table, Some(&op.value)).await?;
        let repo = self.system_repo()?;
        let owned_table = table.clone();
        let owned_op = op.clone();
        let owned_source_op = source_op.clone();
        repo.run_implicit_batch_tx(Actor::System, "complete_table_rename", move |tx| {
            let table = owned_table.clone();
            let op = owned_op.clone();
            let source_op = owned_source_op.clone();
            Box::pin(async move {
                table.execute_set_tx(&op, tx, None, &Actor::System).await?;
                if source_row_present {
                    let refs = crate::types::common::new_map();
                    let ctx = table.info_store_read_view().filter_context(&refs).await?;
                    table
                        .execute_delete_tx(&source_op, &ctx, tx, None, &Actor::System)
                        .await?;
                }
                Ok(crate::query::write::WriteResult {
                    affected: 1,
                    records: Vec::new(),
                    execution_time_us: 0,
                })
            })
        })
        .await
        .map_err(|error| DbError::Internal(error.to_string()))?;
        self.flush_system_repo_durable(&table).await
    }
}
