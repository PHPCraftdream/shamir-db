use crate::core::interner::Interner;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_record_key::IndexRecordKey;
use crate::db::storage::types::Store;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

pub struct TableIndexManager {
    interner: Arc<OnceCell<Interner>>,

    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,

    indexes: Arc<RwLock<IndexInfo>>,
    indexes_unique: Arc<RwLock<IndexInfo>>,

    has_indexes: AtomicBool,
    has_indexes_unique: AtomicBool,
}

impl Clone for TableIndexManager {
    fn clone(&self) -> Self {
        Self {
            interner: Arc::clone(&self.interner),
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            indexes: Arc::clone(&self.indexes),
            indexes_unique: Arc::clone(&self.indexes_unique),
            has_indexes: AtomicBool::new(self.has_indexes.load(Ordering::Relaxed)),
            has_indexes_unique: AtomicBool::new(self.has_indexes_unique.load(Ordering::Relaxed)),
        }
    }
}

impl TableIndexManager {
    pub async fn new(
        data_store: Arc<dyn Store>,
        info_store: Arc<dyn Store>,
        interner: Arc<OnceCell<Interner>>,
    ) -> Result<Self, crate::db::DbError> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();

        let indexes = match info_store.get(indexes_key.clone()).await {
            Ok(bytes) => {
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
            }
            Err(crate::db::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        let indexes_unique = match info_store.get(indexes_unique_key.clone()).await {
            Ok(bytes) => {
                bincode::deserialize::<IndexInfo>(&bytes).unwrap_or_else(|_| IndexInfo::new())
            }
            Err(crate::db::DbError::NotFound(_)) => IndexInfo::new(),
            Err(e) => return Err(e),
        };

        let has_indexes = AtomicBool::new(indexes.is_enabled());
        let has_indexes_unique = AtomicBool::new(indexes_unique.is_enabled());

        Ok(Self {
            interner,
            data_store,
            info_store,
            indexes: Arc::new(RwLock::new(indexes)),
            indexes_unique: Arc::new(RwLock::new(indexes_unique)),
            has_indexes,
            has_indexes_unique,
        })
    }

    pub fn has_indexes(&self) -> bool {
        self.has_indexes.load(Ordering::Relaxed)
    }

    pub fn has_unique_indexes(&self) -> bool {
        self.has_indexes_unique.load(Ordering::Relaxed)
    }

    fn extract_value_by_path(interner: &Interner, value: &InnerValue, path: &[u64]) -> Option<InnerValue> {
        if path.is_empty() {
            return Some(value.clone());
        }

        match value {
            InnerValue::Map(map) => {
                let key = interner.make_key(path[0]);
                let next_value = map.get(&key)?;
                if path.len() == 1 {
                    Some(next_value.clone())
                } else {
                    Self::extract_value_by_path(interner, next_value, &path[1..])
                }
            }
            _ => None,
        }
    }

    fn extract_index_values(interner: &Interner, value: &InnerValue, paths: &[IndexInfoItem]) -> Option<Vec<InnerValue>> {
        let mut values = Vec::with_capacity(paths.len());
        for item in paths {
            match Self::extract_value_by_path(interner, value, &item.path) {
                Some(v) => values.push(v),
                None => return None,
            }
        }
        Some(values)
    }

    fn build_index_key(paths: &[Vec<u64>], values: &[InnerValue]) -> Bytes {
        let value_refs: Vec<&InnerValue> = values.iter().collect();
        IndexRecordKey::new(false, paths.to_vec())
            .with_values(&value_refs)
            .to_bytes()
    }

    async fn add_index_entry(
        &self,
        paths: &[Vec<u64>],
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> crate::db::DbResult<()> {
        let index_key = Self::build_index_key(paths, values);
        let mut key = index_key.to_vec();
        key.extend_from_slice(&record_id.to_bytes());
        self.info_store.set(Bytes::from(key), Bytes::new()).await?;
        Ok(())
    }

    async fn remove_index_entry(
        &self,
        paths: &[Vec<u64>],
        values: &[InnerValue],
        record_id: &RecordId,
    ) -> crate::db::DbResult<()> {
        let index_key = Self::build_index_key(paths, values);
        let mut key = index_key.to_vec();
        key.extend_from_slice(&record_id.to_bytes());
        self.info_store.remove(Bytes::from(key)).await?;
        Ok(())
    }

    pub async fn create_index(&self, index_def: IndexDefinition) -> crate::db::DbResult<()> {
        let interner = match self.interner.get() {
            Some(i) => i,
            None => return Err(crate::db::DbError::Internal("Interner not initialized".to_string())),
        };

        let index_name = index_def.name.clone();
        let paths: Vec<Vec<u64>> = index_def.paths.iter().map(|p| p.path.clone()).collect();
        let paths_ref = &paths;

        let records = self.data_store.iter().await?;

        let mut count = 0usize;
        for (key_bytes, value_bytes) in records {
            let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                Ok(a) => a,
                Err(_) => continue,
            };
            let record_id = RecordId(arr);

            let value = match InnerValue::from_bytes(value_bytes) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(values) = Self::extract_index_values(interner, &value, &index_def.paths) {
                self.add_index_entry(paths_ref, &values, &record_id).await?;
                count += 1;
            }
        }

        {
            let mut indexes = self.indexes.write().await;
            indexes.add_index(index_def);
            self.has_indexes.store(true, Ordering::Release);
        }

        self.save_index_info().await?;

        log::info!("Created index '{}' with {} entries", index_name, count);
        Ok(())
    }

    pub async fn drop_index(&self, name: &str) -> crate::db::DbResult<bool> {
        let paths = {
            let indexes = self.indexes.read().await;
            match indexes.get_index(name) {
                Some(def) => def.paths.iter().map(|p| p.path.clone()).collect::<Vec<_>>(),
                None => return Ok(false),
            }
        };

        let prefix = IndexRecordKey::new(false, paths).to_prefix_bytes();
        let entries = self.info_store.scan_prefix(prefix).await?;

        for (key, _) in entries {
            self.info_store.remove(key).await?;
        }

        let removed = {
            let mut indexes = self.indexes.write().await;
            let was_removed = indexes.remove_index(name);
            self.has_indexes.store(indexes.is_enabled(), Ordering::Release);
            was_removed
        };

        if removed {
            self.save_index_info().await?;
        }

        Ok(removed)
    }

    async fn save_index_info(&self) -> crate::db::DbResult<()> {
        let indexes_key = RecordId::system("indexes").to_bytes();
        let indexes = self.indexes.read().await.clone();
        let bytes = bincode::serialize(&indexes)
            .map_err(|e| crate::db::DbError::Codec(e.to_string()))?;
        self.info_store.set(indexes_key, Bytes::from(bytes)).await?;
        Ok(())
    }

    pub async fn on_record_created(&self, record_id: &RecordId, value: &InnerValue) -> crate::db::DbResult<()> {
        if !self.has_indexes() {
            return Ok(());
        }

        let interner = match self.interner.get() {
            Some(i) => i,
            None => return Err(crate::db::DbError::Internal("Interner not initialized".to_string())),
        };

        let indexes = self.indexes.read().await;
        for def in indexes.definitions() {
            if let Some(values) = Self::extract_index_values(interner, value, &def.paths) {
                let paths: Vec<Vec<u64>> = def.paths.iter().map(|p| p.path.clone()).collect();
                self.add_index_entry(&paths, &values, record_id).await?;
            }
        }

        Ok(())
    }

    pub async fn on_record_updated(
        &self,
        record_id: &RecordId,
        old_value: &InnerValue,
        new_value: &InnerValue,
    ) -> crate::db::DbResult<()> {
        if !self.has_indexes() {
            return Ok(());
        }

        let interner = match self.interner.get() {
            Some(i) => i,
            None => return Err(crate::db::DbError::Internal("Interner not initialized".to_string())),
        };

        let indexes = self.indexes.read().await;
        for def in indexes.definitions() {
            let old_values = Self::extract_index_values(interner, old_value, &def.paths);
            let new_values = Self::extract_index_values(interner, new_value, &def.paths);
            let paths: Vec<Vec<u64>> = def.paths.iter().map(|p| p.path.clone()).collect();

            match (old_values, new_values) {
                (None, None) => {}
                (None, Some(new)) => {
                    self.add_index_entry(&paths, &new, record_id).await?;
                }
                (Some(old), None) => {
                    self.remove_index_entry(&paths, &old, record_id).await?;
                }
                (Some(old), Some(new)) => {
                    if old != new {
                        self.remove_index_entry(&paths, &old, record_id).await?;
                        self.add_index_entry(&paths, &new, record_id).await?;
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn on_record_deleted(&self, record_id: &RecordId, old_value: &InnerValue) -> crate::db::DbResult<()> {
        if !self.has_indexes() {
            return Ok(());
        }

        let interner = match self.interner.get() {
            Some(i) => i,
            None => return Err(crate::db::DbError::Internal("Interner not initialized".to_string())),
        };

        let indexes = self.indexes.read().await;
        for def in indexes.definitions() {
            if let Some(values) = Self::extract_index_values(interner, old_value, &def.paths) {
                let paths: Vec<Vec<u64>> = def.paths.iter().map(|p| p.path.clone()).collect();
                self.remove_index_entry(&paths, &values, record_id).await?;
            }
        }

        Ok(())
    }
}
