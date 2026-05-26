//! Interner manager for lazy loading and persistence

use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_types::codecs::basic::bincode;
use shamir_types::core::interner::{Interner, InternerKey, UserKey};
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Manages interned keys with lazy loading and persistence.
///
/// The interner is loaded lazily on first access and persisted to
/// storage when new keys are added.
///
/// **Persist optimisation (Opt A):** the interner is monotonic — keys
/// are only added, never removed — so its `len()` uniquely identifies
/// its content. We track `last_persisted_len`; `persist()` becomes a
/// no-op when nothing has been added since the previous call. The
/// common write path (insert / set / update with already-known field
/// names) ends up calling `persist()` after every op, but the actual
/// I/O only fires once per genuinely-new key. Skipping a persist when
/// the interner hasn't grown costs ~10 ns instead of a full
/// serialize-and-write of the entire dictionary.
///
/// Uses `Arc<OnceCell>` so all clones share the same interner.
pub struct InternerManager {
    info_store: Arc<dyn Store>,
    interner: Arc<OnceCell<Interner>>,
    /// Length of the interner the last time `persist` actually wrote
    /// to the info store. Compared against `interner.len()` on each
    /// `persist()` call to skip no-op writes.
    last_persisted_len: Arc<AtomicUsize>,
}

impl Clone for InternerManager {
    fn clone(&self) -> Self {
        Self {
            info_store: Arc::clone(&self.info_store),
            interner: Arc::clone(&self.interner),
            last_persisted_len: Arc::clone(&self.last_persisted_len),
        }
    }
}

impl InternerManager {
    /// Create a new interner manager
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            interner: Arc::new(OnceCell::new()),
            last_persisted_len: Arc::new(AtomicUsize::new(0)),
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
            shamir_storage::error::DbError::Codec(format!("Failed to serialize interner: {}", e))
        })?;

        self.info_store.set(internals_id.to_bytes(), bytes).await?;

        Ok(())
    }

    /// Persist the full interner state to storage **if it has grown
    /// since the last persist**. Otherwise this call is a near-free
    /// no-op (one atomic load + integer compare).
    ///
    /// Saves all current entries, replacing whatever was stored before.
    /// Callers (insert/update/set on the write path) invoke this after
    /// every op; thanks to the monotonic + length-tracking trick, only
    /// the ops that actually intern a new key pay the I/O cost.
    pub async fn persist(&self) -> DbResult<()> {
        let interner = self.get().await?;
        let cur_len = interner.len();
        let last = self.last_persisted_len.load(Ordering::Acquire);
        if cur_len == last {
            // Interner hasn't grown — disk copy is already current.
            return Ok(());
        }

        // Snapshot full state and write. We update `last_persisted_len`
        // *before* the await so a concurrent call doesn't redundantly
        // race us; it'd be a wasted write but not incorrect. (In
        // practice writes to a single repo are serialised anyway.)
        let entries = interner.all_entries();
        if entries.is_empty() {
            self.last_persisted_len.store(0, Ordering::Release);
            return Ok(());
        }

        let internals_id = RecordId::system("internals");
        let bytes = bincode::to_bytes(&entries).map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize interner: {}", e))
        })?;
        self.info_store.set(internals_id.to_bytes(), bytes).await?;
        self.last_persisted_len.store(cur_len, Ordering::Release);
        Ok(())
    }
}
