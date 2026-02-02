use crate::db::storage::types::Store;
use crate::db::error::{DbError, DbResult};
use crate::types::value::{InnerValue, Value};
use crate::types::common::{new_dash_map_wc, TDashMap};
use crate::types::record_id::RecordId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A persistent version of the Interner that uses a Store for durability.
pub struct PersistentInterner {
    store: Arc<dyn Store>,
    map_str: TDashMap<String, u64>,
    map_ind: TDashMap<u64, String>,
    current: AtomicU64,
}

impl PersistentInterner {
    /// Creates a new PersistentInterner using the provided store.
    /// It will load all existing mappings from the store.
    pub async fn load(store: Arc<dyn Store>) -> DbResult<Self> {
        let map_str = new_dash_map_wc(1024);
        let map_ind = new_dash_map_wc(1024);
        let mut max_id = 0;

        // Load existing mappings
        let records = store.iter().await?;
        for record in records {
            // record.0 is RecordId (which we use to store u64 index)
            // record.3 is InnerValue (which should be Str)
            
            let id = record_id_to_u64(record.0);
            if let Value::Str(s) = record.3 {
                map_str.insert(s.clone(), id);
                map_ind.insert(id, s);
                if id > max_id {
                    max_id = id;
                }
            }
        }

        Ok(Self {
            store,
            map_str,
            map_ind,
            current: AtomicU64::new(max_id + 1),
        })
    }

    /// Gets the ID for a string, creating and persisting it if it doesn't exist.
    pub async fn touch_ind<S: AsRef<str>>(&self, s: S) -> DbResult<u64> {
        let key = s.as_ref();
        
        // 1. Check memory first
        if let Some(id) = self.map_str.get(key) {
            return Ok(*id);
        }

        // 2. Not in memory, need to create new ID and persist it
        // Note: Using a simple lock or coordination might be needed for high concurrency
        // but for now let's rely on atomic increment and store.set.
        
        let new_id = self.current.fetch_add(1, Ordering::SeqCst);
        let record_id = u64_to_record_id(new_id);
        let value = InnerValue::Str(key.to_string());

        // Persist to store
        self.store.set(record_id, &value).await?;

        // Update memory
        self.map_str.insert(key.to_string(), new_id);
        self.map_ind.insert(new_id, key.to_string());

        Ok(new_id)
    }

    /// Gets the string corresponding to an ID.
    pub fn get_str(&self, index: u64) -> Option<String> {
        self.map_ind.get(&index).map(|s| s.clone())
    }

    /// Gets the ID corresponding to a string.
    pub fn get_ind<S: AsRef<str>>(&self, s: S) -> Option<u64> {
        self.map_str.get(s.as_ref()).map(|id| *id)
    }
}

// Helpers to convert between u64 and RecordId for storage
fn u64_to_record_id(val: u64) -> RecordId {
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&val.to_le_bytes());
    RecordId(bytes)
}

fn record_id_to_u64(id: RecordId) -> u64 {
    let bytes: [u8; 8] = id.0[0..8].try_into().unwrap_or([0u8; 8]);
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_sled::SledRepo;
    use crate::db::storage::types::Repo;
    use std::fs;

    #[tokio::test]
    async fn test_persistent_interner() {
        let path = "./test_data/test_persistent_interner";
        if std::path::Path::new(path).exists() {
            fs::remove_dir_all(path).unwrap();
        }

        let repo = SledRepo::new(path).unwrap();
        let store = repo.store_get("interner").await.unwrap();

        {
            let interner = PersistentInterner::load(store.clone()).await.unwrap();
            let id1 = interner.touch_ind("name").await.unwrap();
            let id2 = interner.touch_ind("age").await.unwrap();
            assert_eq!(id1, 1);
            assert_eq!(id2, 2);
            assert_eq!(interner.get_str(1), Some("name".to_string()));
        }

        // Reload from same store
        {
            let interner = PersistentInterner::load(store).await.unwrap();
            assert_eq!(interner.get_str(1), Some("name".to_string()));
            assert_eq!(interner.get_ind("age"), Some(2));
            
            let id3 = interner.touch_ind("city").await.unwrap();
            assert_eq!(id3, 3);
        }
    }
}
