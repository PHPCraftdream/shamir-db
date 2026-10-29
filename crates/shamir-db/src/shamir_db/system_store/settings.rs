use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::engine::table::TableManager;
use crate::DbResult;

use super::store::{
    qv_map1, TABLE_PUBLICATIONS, TABLE_REPLICATION_PROFILES, TABLE_ROLES, TABLE_SETTINGS,
    TABLE_SUBSCRIPTIONS, TABLE_USERS,
};
use super::SystemStore;

impl SystemStore {
    pub(crate) async fn load_invocation_settings(
        &self,
    ) -> DbResult<(Option<QueryValue>, Option<QueryValue>)> {
        #[cfg(test)]
        self.invocation_settings_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let table = self.table(TABLE_SETTINGS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_SETTINGS).filter(
            crate::query::filter::Filter::Or {
                filters: ["root_meta", "fn_namespace_meta"]
                    .iter()
                    .map(|key| crate::query::filter::Filter::Eq {
                        field: vec!["key".to_string()],
                        value: crate::query::filter::FilterValue::String((*key).to_string()),
                    })
                    .collect(),
            },
        );
        let result = table.read(&query, &ctx).await?;
        if !result.corrupt_records.is_empty() {
            return Err(crate::DbError::Internal(
                "corrupt settings catalogue".to_string(),
            ));
        }
        let mut root = None;
        let mut namespace = None;
        for record in result.records {
            let key = record.get_value_owned("key");
            match key.as_ref().and_then(QueryValue::as_str) {
                Some("root_meta") if root.is_none() => root = record.get_value_owned("value"),
                Some("fn_namespace_meta") if namespace.is_none() => {
                    namespace = record.get_value_owned("value");
                }
                _ => {}
            }
        }
        Ok((root, namespace))
    }
    /// Save a setting.
    pub async fn save_setting(&self, key: &str, value: &QueryValue) -> DbResult<()> {
        let table = self.table(TABLE_SETTINGS).await?;
        let mut rec_m = new_map();
        rec_m.insert("key".to_string(), QueryValue::Str(key.to_string()));
        rec_m.insert("value".to_string(), value.clone());
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_SETTINGS),
            key: qv_map1("key", QueryValue::Str(key.to_string())),
            value: QueryValue::Map(rec_m),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        self.persist_table_interner(&table, None).await?;
        Ok(())
    }

    /// Load a setting.
    pub async fn load_setting(&self, key: &str) -> DbResult<Option<QueryValue>> {
        #[cfg(test)]
        self.invocation_settings_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let table = self.table(TABLE_SETTINGS).await?;
        let refs = crate::types::common::new_map();
        let ctx = table.info_store_read_view().filter_context(&refs).await?;
        let query = crate::query::read::ReadQuery::new(TABLE_SETTINGS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["key".to_string()],
                value: crate::query::filter::FilterValue::String(key.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .and_then(|r| r.get_value_owned("value")))
    }

    // Users & Roles (tables ready, API to be implemented)

    /// Get the users table manager.
    pub async fn users_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_USERS).await
    }

    /// Get the roles table manager.
    pub async fn roles_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_ROLES).await
    }

    // Replication catalogue (386-a) — profiles / publications / subscriptions

    /// Get the replication-profiles table manager.
    pub async fn replication_profiles_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_REPLICATION_PROFILES).await
    }

    /// Get the publications table manager.
    pub async fn publications_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_PUBLICATIONS).await
    }

    /// Get the subscriptions table manager.
    pub async fn subscriptions_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_SUBSCRIPTIONS).await
    }

    // Function catalogue (slice 4)
}
