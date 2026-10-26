use shamir_types::access::{Actor, ResourceMeta};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::{DbError, DbResult};

use super::store::{qv_map1, TABLE_FUNCTIONS, TABLE_GROUPS, TABLE_USERS};
use super::SystemStore;

impl SystemStore {
    /// Persist a function catalogue entry. Upsert keyed by `name`.
    /// Injects `owner`/`group`/`mode` from `meta` into the record.
    pub async fn save_function(
        &self,
        name: &str,
        record: &QueryValue,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTIONS),
            key: qv_map1("name", QueryValue::Str(name.to_string())),
            value: rec,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        // Durable DDL — sync_wal + flush via shared helper.
        self.flush_system_repo_durable(&table).await?;
        Ok(())
    }

    /// Remove a function catalogue entry by name.
    pub async fn remove_function(&self, name: &str) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_FUNCTIONS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — sync_wal + flush via shared helper.
        self.flush_system_repo_durable(&table).await?;
        Ok(())
    }

    /// Load every persisted user record (including `password_hash` —
    /// callers that surface these must strip secret fields themselves).
    pub async fn load_users(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_USERS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_USERS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load every persisted function catalogue record.
    pub async fn load_functions(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTIONS);
        let result = table.read(&query, &ctx).await?;
        if !result.corrupt_records.is_empty() {
            return Err(DbError::Internal(format!(
                "corrupt function catalogue: {} unreadable record(s)",
                result.corrupt_records.len()
            )));
        }
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load a single function catalogue record by name.
    pub async fn load_function(&self, name: &str) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTIONS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        if !result.corrupt_records.is_empty() {
            return Err(DbError::Internal(format!(
                "corrupt function catalogue: {} unreadable record(s)",
                result.corrupt_records.len()
            )));
        }
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    // Groups store (P3 metadata plates)

    /// Persist a group record. `group_id` is allocated by the caller
    /// (see [`ShamirDb::create_group`]). `owner` is the persist-friendly
    /// `Actor::to_owner_id()` encoding of the group's owner (task #552) —
    /// callers doing a read-modify-write (`add_group_member`/
    /// `remove_group_member` below, and `access_control.rs`'s
    /// `rename_group_as`) must thread the EXISTING record's owner through
    /// unchanged; only `create_group_as` and the `Group` `set_resource_meta`
    /// write arm intentionally set a NEW owner.
    pub async fn save_group(
        &self,
        group_id: u64,
        name: &str,
        members: &[u64],
        owner: u64,
    ) -> DbResult<()> {
        self.catalogue_write_fault_gate()?;
        let members_list: Vec<QueryValue> =
            members.iter().map(|&m| QueryValue::Int(m as i64)).collect();
        let mut m = new_map();
        m.insert("group_id".to_string(), QueryValue::Int(group_id as i64));
        m.insert("name".to_string(), QueryValue::Str(name.to_string()));
        m.insert("members".to_string(), QueryValue::List(members_list));
        m.insert("owner".to_string(), QueryValue::Int(owner as i64));
        let record = QueryValue::Map(m);
        let table = self.table(TABLE_GROUPS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_GROUPS),
            key: qv_map1("group_id", QueryValue::Int(group_id as i64)),
            value: record,
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    /// Load all group records.
    pub async fn load_groups(&self) -> DbResult<Vec<QueryValue>> {
        let table = self.table(TABLE_GROUPS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_GROUPS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| r.as_value().into_owned())
            .collect())
    }

    /// Load a single group record by id.
    pub async fn load_group(&self, group_id: u64) -> DbResult<Option<QueryValue>> {
        let table = self.table(TABLE_GROUPS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_GROUPS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["group_id".to_string()],
                value: crate::query::filter::FilterValue::Int(group_id as i64),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| r.as_value().into_owned()))
    }

    /// Add a user to a group. Reads the group, appends the user, and
    /// re-persists. Returns `Ok(())` even if the user was already a member.
    ///
    /// Read-modify-write: threads the existing record's `owner` field
    /// through unchanged (task #552) — adding a member must not touch
    /// ownership.
    pub async fn add_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        let rec = self
            .load_group(group_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("group id {} not found", group_id)))?;
        let mut members: Vec<u64> = rec
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();
        if !members.contains(&user_id) {
            members.push(user_id);
        }
        let name = rec.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        let owner = ResourceMeta::owner_field(&rec)
            .unwrap_or(Actor::System)
            .to_owner_id();
        self.save_group(group_id, name, &members, owner).await
    }

    /// Remove a user from a group. Returns `Ok(())` even if the user was
    /// not a member.
    ///
    /// Read-modify-write: threads the existing record's `owner` field
    /// through unchanged (task #552) — removing a member must not touch
    /// ownership.
    pub async fn remove_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        let rec = self
            .load_group(group_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("group id {} not found", group_id)))?;
        let mut members: Vec<u64> = rec
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();
        members.retain(|&m| m != user_id);
        let name = rec.get("name").and_then(|v| v.as_str()).unwrap_or_default();
        let owner = ResourceMeta::owner_field(&rec)
            .unwrap_or(Actor::System)
            .to_owner_id();
        self.save_group(group_id, name, &members, owner).await
    }

    /// Update a group's owner in place. Reloads the existing `name` +
    /// `members`, then re-persists with the new `owner` — the write path
    /// for the `Group` `set_resource_meta` arm (task #552). Group `mode`
    /// stays fixed/computed (`0o750`); only `owner` is settable this way.
    pub async fn set_group_owner(&self, group_id: u64, owner: u64) -> DbResult<()> {
        let rec = self
            .load_group(group_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("group id {} not found", group_id)))?;
        let name = rec
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let members: Vec<u64> = rec
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();
        self.save_group(group_id, &name, &members, owner).await
    }

    /// Remove a group record by id.
    pub async fn remove_group(&self, group_id: u64) -> DbResult<()> {
        let table = self.table(TABLE_GROUPS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_GROUPS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["group_id".to_string()],
                value: crate::query::filter::FilterValue::Int(group_id as i64),
            },
            select: None,
            expected_version: None,
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        table.data_store_read_view().flush().await?;
        Ok(())
    }

    // Catalogue lookups for resource_meta resolver
}
