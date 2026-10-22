use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};

use crate::meta::MetaEnvelope;
use crate::table::TableConfig;

const INVENTORY_KEY: &[u8] = b"__meta__/seed_install/inventory";

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
}
