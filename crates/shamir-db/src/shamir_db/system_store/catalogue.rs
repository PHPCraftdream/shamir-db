use shamir_types::access::ResourceMeta;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::DbResult;

use super::store::{qv_map1, qv_map2, TABLE_DATABASES, TABLE_REPOSITORIES, TABLE_TABLES};
use super::SystemStore;

impl SystemStore {
    /// Save database metadata. Injects the `owner`/`group`/`mode` fields
    /// from `meta` into the record before persisting (P3 metadata plates).
    pub async fn save_database(
        &self,
        name: &str,
        record: &QueryValue,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_DATABASES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_DATABASES),
            key: qv_map1("name", QueryValue::Str(name.to_string())),
            value: rec.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, Some(&rec)).await?;
        Ok(())
    }

    /// Remove database metadata.
    pub async fn remove_database(&self, name: &str) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_DATABASES),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        Ok(())
    }

    /// Load all database records.
    pub async fn load_databases(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_DATABASES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_DATABASES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    // Repository metadata

    /// Save repository metadata. Injects `owner`/`group`/`mode` from `meta`
    /// and, when `Some`, the per-repo durability override under the
    /// `durability` key (encoded by the `repo_durability` codec).
    pub async fn save_repository(
        &self,
        db_name: &str,
        repo_name: &str,
        engine: &str,
        path: Option<&str>,
        meta: &ResourceMeta,
        durability: Option<&shamir_query_types::durability::DurabilityOverride>,
    ) -> DbResult<()> {
        let record = crate::shamir_db::repo_durability::repository_record(
            db_name, repo_name, engine, path, meta, durability,
        );
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_REPOSITORIES),
            key: qv_map2(
                "db_name",
                QueryValue::Str(db_name.to_string()),
                "repo_name",
                QueryValue::Str(repo_name.to_string()),
            ),
            value: record,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        // DDL must be durable immediately: flush the MemBuffer-wrapped
        // store so a crash right after the admin op can't lose (or, for
        // removes, resurrect) the catalogue entry. DDL is rare → the
        // fsync cost is irrelevant.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Remove repository metadata.
    pub async fn remove_repository(&self, db_name: &str, repo_name: &str) -> DbResult<()> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_REPOSITORIES),
            where_clause: crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                ],
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Load all repository records.
    pub async fn load_repositories(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_REPOSITORIES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load a single repository's catalogue record by `(db_name, repo_name)`.
    /// Returns `Ok(None)` when no matching row exists. Used by rename_repo to
    /// preserve `engine` / `path` / `ResourceMeta` across the re-key.
    pub async fn load_repository_record(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<Option<QueryValue>> {
        let all = self.load_repositories().await?;
        for rec in all {
            let matches = rec
                .get("db_name")
                .and_then(|v| v.as_str())
                .is_some_and(|d| d == db_name)
                && rec
                    .get("repo_name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|r| r == repo_name);
            if matches {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    // Table catalogue (per-repo table list — I.2)

    /// Persist one table's catalogue entry. Keyed by
    /// `(db_name, repo_name, table_name)` so re-saving the same table is an
    /// idempotent upsert. `enable_indexes` is the only other field of
    /// `TableConfig`, so the record carries enough to faithfully re-create
    /// the table on open.
    pub async fn save_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut m = new_map();
        m.insert("db_name".to_string(), QueryValue::Str(db_name.to_string()));
        m.insert(
            "repo_name".to_string(),
            QueryValue::Str(repo_name.to_string()),
        );
        m.insert(
            "table_name".to_string(),
            QueryValue::Str(table_name.to_string()),
        );
        m.insert(
            "enable_indexes".to_string(),
            QueryValue::Bool(enable_indexes),
        );
        let mut record = QueryValue::Map(m);
        meta.inject_into(&mut record);
        let table = self.table(TABLE_TABLES).await?;
        let mut key_m = new_map();
        key_m.insert("db_name".to_string(), QueryValue::Str(db_name.to_string()));
        key_m.insert(
            "repo_name".to_string(),
            QueryValue::Str(repo_name.to_string()),
        );
        key_m.insert(
            "table_name".to_string(),
            QueryValue::Str(table_name.to_string()),
        );
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: QueryValue::Map(key_m),
            value: record,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Remove one table's catalogue entry.
    pub async fn remove_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<()> {
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_TABLES),
            where_clause: crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["table_name".to_string()],
                        value: crate::query::filter::FilterValue::String(table_name.to_string()),
                    },
                ],
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Load every persisted table-catalogue record (across all repos). The
    /// caller filters by `db_name` / `repo_name`.
    pub async fn load_tables(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_TABLES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_TABLES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    // Settings

    // ---------------------------------------------------------------------------
    // Record lookups
    // ---------------------------------------------------------------------------

    /// Load a single database record by name.
    pub async fn load_database(&self, name: &str) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_DATABASES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_DATABASES).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    /// Load a single repository record by (db_name, repo_name).
    pub async fn load_repository(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_REPOSITORIES).filter(
            crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                ],
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    /// Load a single table catalogue record by (db, repo, table).
    pub async fn load_table_record(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_TABLES).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_TABLES).filter(
            crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["table_name".to_string()],
                        value: crate::query::filter::FilterValue::String(table_name.to_string()),
                    },
                ],
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    /// Persist a replacement database record (for `set_resource_meta`).
    pub async fn save_database_meta(&self, name: &str, record: &QueryValue) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_DATABASES),
            key: qv_map1("name", QueryValue::Str(name.to_string())),
            value: record.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, Some(record)).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Persist a replacement repository record (for `set_resource_meta`).
    pub async fn save_repository_meta(&self, record: &QueryValue) -> DbResult<()> {
        let db_name = record
            .get("db_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let repo_name = record
            .get("repo_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_REPOSITORIES),
            key: qv_map2(
                "db_name",
                QueryValue::Str(db_name),
                "repo_name",
                QueryValue::Str(repo_name),
            ),
            value: record.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Persist a replacement table catalogue record (for `set_resource_meta`).
    pub async fn save_table_meta(&self, record: &QueryValue) -> DbResult<()> {
        let db_name = record
            .get("db_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let repo_name = record
            .get("repo_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let table_name = record
            .get("table_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let table = self.table(TABLE_TABLES).await?;
        let mut key_m = new_map();
        key_m.insert("db_name".to_string(), QueryValue::Str(db_name));
        key_m.insert("repo_name".to_string(), QueryValue::Str(repo_name));
        key_m.insert("table_name".to_string(), QueryValue::Str(table_name));
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: QueryValue::Map(key_m),
            value: record.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    // Validator catalogue (S1)
}
