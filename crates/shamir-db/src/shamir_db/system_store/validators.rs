use shamir_types::access::ResourceMeta;
use shamir_types::types::value::QueryValue;

use crate::DbResult;

use super::store::{qv_map1, TABLE_FUNCTIONS, TABLE_FUNCTION_FOLDERS, TABLE_VALIDATORS};
use super::SystemStore;

impl SystemStore {
    /// Persist a validator catalogue entry. Upsert keyed by `name`.
    /// Injects `owner`/`group`/`mode` from `meta` into the record.
    pub async fn save_validator(
        &self,
        name: &str,
        record: &QueryValue,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_VALIDATORS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_VALIDATORS),
            key: qv_map1("name", QueryValue::Str(name.to_string())),
            value: rec,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Remove a validator catalogue entry by name.
    pub async fn remove_validator(&self, name: &str) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let table = self.table(TABLE_VALIDATORS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_VALIDATORS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Load every persisted validator catalogue record.
    pub async fn load_validators(&self) -> DbResult<Vec<QueryValue>> {
        #[cfg(test)]
        self.count_validator_catalogue_read();
        let table = self.table(TABLE_VALIDATORS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_VALIDATORS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load a single validator catalogue record by name.
    pub async fn load_validator(&self, name: &str) -> DbResult<Option<QueryValue>> {
        #[cfg(test)]
        self.count_validator_catalogue_read();
        let table = self.table(TABLE_VALIDATORS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_VALIDATORS).filter(
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

    /// Persist a replacement function catalogue record (for `set_resource_meta`).
    pub async fn save_function_meta_record(&self, name: &str, record: &QueryValue) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTIONS),
            key: qv_map1("name", QueryValue::Str(name.to_string())),
            value: record.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    // Function folder catalogue (#118)

    /// Persist a function folder catalogue entry. Upsert keyed by `path`
    /// (slash-joined, e.g. `"reports/daily"`). Injects `owner`/`group`/`mode`
    /// from `meta` into the record.
    pub async fn save_function_folder(
        &self,
        path_key: &str,
        record: &QueryValue,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            key: qv_map1("path", QueryValue::Str(path_key.to_string())),
            value: rec,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Remove a function folder catalogue entry by path key.
    pub async fn remove_function_folder(&self, path_key: &str) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["path".to_string()],
                value: crate::query::filter::FilterValue::String(path_key.to_string()),
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Load every persisted function folder catalogue record.
    pub async fn load_function_folders(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTION_FOLDERS);
        let result = table.read(&query, &ctx).await?;
        if !result.corrupt_records.is_empty() {
            return Err(crate::DbError::Internal(format!(
                "corrupt function folder catalogue: {} corrupt records",
                result.corrupt_records.len()
            )));
        }
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load a single function folder catalogue record by path key.
    pub async fn load_function_folder(&self, path_key: &str) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTION_FOLDERS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["path".to_string()],
                value: crate::query::filter::FilterValue::String(path_key.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        if !result.corrupt_records.is_empty() {
            return Err(crate::DbError::Internal(format!(
                "corrupt function folder {path_key} catalogue: {} corrupt records",
                result.corrupt_records.len()
            )));
        }
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    /// Canonical-join the borrowed segments once and delegate to the keyed
    /// `Eq`-filtered lookup — no full-catalogue scan on the authorization
    /// ancestor path. (FunctionFolderPrefix callers use the borrowed
    /// `&str` key of `load_function_folder` directly.)
    ///
    /// L3-RV-TYPES-A7B1FCEC (b) — known allocation: the single
    /// `path.join("/")` per ancestor lookup cannot be borrowed away. The
    /// folder record's stored identity IS the joined string: it is written
    /// as the `path` field (`save_function_folder_meta` →
    /// `qv_map1("path", path_key.to_string())`) and read back via
    /// `Filter::Eq { field: ["path"], value: FilterValue::String(..) }` in
    /// `load_function_folder` — the filter value is an owned `String`, and
    /// the read path offers no borrowed-match API. The join here is
    /// once-per-ancestor-lookup (one allocation per call, the call already
    /// receives the whole ancestor slice), never per-segment: a deep chain
    /// of S segments costs S-1 joins total across the authorization
    /// traversal. Each join copies its whole prefix, so total copied bytes
    /// grow quadratically with depth; folder depth is admin-bounded and the
    /// allocation COUNT stays one per ancestor.
    pub async fn load_function_folder_path(&self, path: &[String]) -> DbResult<Option<QueryValue>> {
        #[cfg(test)]
        self.count_folder_path_lookup();
        let canonical = path.join("/");
        self.load_function_folder(&canonical).await
    }

    /// Persist a replacement function folder record (for `set_resource_meta`).
    pub async fn save_function_folder_meta(
        &self,
        path_key: &str,
        record: &QueryValue,
    ) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            key: qv_map1("path", QueryValue::Str(path_key.to_string())),
            value: record.clone(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }
}
