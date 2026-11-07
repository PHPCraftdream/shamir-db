use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_collections::TFxSet;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;

use crate::meta::MetaEnvelope;
use crate::table::TableConfig;

const INVENTORY_KEY: &[u8] = b"__meta__/seed_install/inventory";
const LOCAL_IDS_KEY: &[u8] = b"__meta__/seed_install/local_table_ids";

/// Engine-owned normalized inventory, including empty tables. Declarative
/// schemas belong to shamir-db; that owner recompiles paths by name and must
/// supply its metadata stores to the writer's completion durability barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedTable {
    pub name: String,
    pub live_records: u64,
}

impl SeedTable {
    pub fn config(&self) -> TableConfig {
        TableConfig::new(self.name.clone())
    }

    /// Load the inventory together with its optional node-local store mapping.
    /// An absent mapping is the legacy, id-less inventory representation.
    pub async fn load_configs(store: &Arc<dyn Store>) -> DbResult<Vec<TableConfig>> {
        let tables = Self::load_inventory(store).await?;
        let bytes = match store.get(RecordKey::from_slice(LOCAL_IDS_KEY)).await {
            Ok(bytes) => bytes,
            Err(DbError::NotFound(_)) => {
                return Ok(tables.iter().map(Self::config).collect());
            }
            Err(error) => return Err(error),
        };
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(DbError::Codec(
                "seed local table IDs exceed metadata budget".into(),
            ));
        }
        let ids: Vec<(String, RecordId)> = MetaEnvelope::open(&bytes)
            .map_err(|e| DbError::Codec(format!("seed local table IDs: {e}")))?;
        if ids.len() != tables.len() {
            return Err(DbError::Codec("seed local table ID count mismatch".into()));
        }
        let mut configs = Vec::with_capacity(tables.len());
        for (table, (name, id)) in tables.iter().zip(ids) {
            if table.name != name {
                return Err(DbError::Codec("seed local table ID name mismatch".into()));
            }
            configs.push(TableConfig::new(name).with_store_id(id));
        }
        Ok(configs)
    }

    pub async fn load_inventory(store: &Arc<dyn Store>) -> DbResult<Vec<Self>> {
        let bytes = store.get(RecordKey::from_slice(INVENTORY_KEY)).await?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(DbError::Codec(
                "seed inventory exceeds metadata budget".into(),
            ));
        }
        let tables: Vec<Self> = MetaEnvelope::open(&bytes)
            .map_err(|e| DbError::Codec(format!("seed inventory: {e}")))?;
        if tables.len() > 65_536 {
            return Err(DbError::Codec("seed inventory exceeds count budget".into()));
        }
        let mut seen = TFxSet::default();
        if tables.iter().any(|table| !seen.insert(table.name.as_str())) {
            return Err(DbError::Codec("duplicate seed inventory table name".into()));
        }
        Ok(tables)
    }

    pub(super) async fn save_inventory(store: &Arc<dyn Store>, tables: &[Self]) -> DbResult<()> {
        let bytes = MetaEnvelope::new(tables.to_vec())
            .encode()
            .map_err(|e| DbError::Codec(format!("seed inventory: {e}")))?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(DbError::Codec(
                "seed inventory exceeds metadata budget".into(),
            ));
        }
        store
            .set(RecordKey::from_slice(INVENTORY_KEY), Bytes::from(bytes))
            .await?;
        store.flush().await
    }

    pub(super) async fn save_local_ids(
        store: &Arc<dyn Store>,
        tables: &[Self],
        configs: &[TableConfig],
    ) -> DbResult<()> {
        if tables.len() != configs.len()
            || tables
                .iter()
                .zip(configs)
                .any(|(table, config)| table.name != config.name)
        {
            return Err(DbError::Codec("seed local table ID names mismatch".into()));
        }
        let ids: Vec<(String, RecordId)> = configs
            .iter()
            .map(|config| {
                config
                    .store_id
                    .as_ref()
                    .map(|id| (config.name.clone(), *id))
                    .ok_or_else(|| DbError::Codec("seed table has no local store ID".into()))
            })
            .collect::<DbResult<_>>()?;
        let bytes = MetaEnvelope::new(ids)
            .encode()
            .map_err(|e| DbError::Codec(format!("seed local table IDs: {e}")))?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(DbError::Codec(
                "seed local table IDs exceed metadata budget".into(),
            ));
        }
        store
            .set(RecordKey::from_slice(LOCAL_IDS_KEY), Bytes::from(bytes))
            .await?;
        store.flush().await
    }
}
