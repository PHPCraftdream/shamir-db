//! Interner manager for lazy loading and persistence

use crate::codecs::basic::bincode;
use crate::core::interner::{Interner, InternerKey, UserKey};
use crate::db::storage::types::Store;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Manages interned keys with lazy loading and persistence
///
/// The interner is loaded lazily on first access and persisted
/// to storage when new keys are added.
///
/// Uses `Arc<OnceCell>` so all clones share the same interner.
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: Arc<OnceCell<Interner>>,
}

impl Clone for InternerManager {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            interner: Arc::clone(&self.interner),
        }
    }
}

impl InternerManager {
    /// Create a new interner manager
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            interner: Arc::new(OnceCell::new()),
        }
    }

    /// Get interner, loading it lazily on first access
    pub async fn get(&self) -> DbResult<&Interner> {
        if self.interner.get().is_some() {
            return Ok(self.interner.get().unwrap());
        }

        // Clone store for async block
        let info_store = Arc::clone(&self.info_store);
        let interner_cell = &self.interner;

        interner_cell
            .get_or_init(|| async move {
                // Load from storage
                let internals_id = RecordId::system("internals").to_bytes();
                let inter_data = info_store.get(internals_id).await;

                if let Ok(bytes) = inter_data {
                    // Deserialize
                    let data: Vec<(InternerKey, UserKey)> = bincode::from_bytes(&bytes)
                        .unwrap_or_else(|e| {
                            log::error!("Failed to deserialize interner: {}", e);
                            Vec::new()
                        });
                    Interner::with_state(data)
                } else {
                    // Empty interner
                    Interner::new()
                }
            })
            .await;

        Ok(self.interner.get().unwrap())
    }

    /// Save new interned keys to storage
    pub async fn save_new_keys(&self, new_keys: &[(InternerKey, UserKey)]) -> DbResult<()> {
        if new_keys.is_empty() {
            return Ok(());
        }

        // Read existing
        let internals_id = RecordId::system("internals");
        let existing = self.info_store.get(internals_id.to_bytes()).await;
        let mut current: Vec<(InternerKey, UserKey)> = if let Ok(bytes) = existing {
            bincode::from_bytes(&bytes).unwrap_or_default()
        } else {
            Vec::new()
        };

        // Add new keys
        current.extend_from_slice(new_keys);

        // Serialize and save
        let bytes = bincode::to_bytes(&current).map_err(|e| {
            crate::db::DbError::Codec(format!("Failed to serialize interner: {}", e))
        })?;

        self.info_store.set(internals_id.to_bytes(), bytes).await?;

        Ok(())
    }

    /// Persist the full interner state to storage.
    ///
    /// Saves all current entries, replacing whatever was stored before.
    /// Call this after operations that may have interned new keys
    /// (e.g., insert, update, set).
    pub async fn persist(&self) -> DbResult<()> {
        let interner = self.get().await?;
        let entries = interner.all_entries();
        if entries.is_empty() {
            return Ok(());
        }

        let internals_id = RecordId::system("internals");
        let bytes = bincode::to_bytes(&entries).map_err(|e| {
            crate::db::DbError::Codec(format!("Failed to serialize interner: {}", e))
        })?;
        self.info_store.set(internals_id.to_bytes(), bytes).await?;
        Ok(())
    }
}
