//! `Persistable` trait and `PersistRegistry` for unified metadata flushing.
//!
//! Any metadata blob that maintains a dirty flag and can flush itself to
//! durable storage implements `Persistable`. `PersistRegistry` holds a
//! collection of such blobs and flushes them all in one call — replacing
//! the scattered paired calls to `interner().persist()` /
//! `counter().persist()` that used to litter the write path.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_storage::error::DbResult;

/// A metadata blob that tracks its own dirty state and can flush to
/// durable storage. Implementations must be cheap when clean (no-op).
#[async_trait]
pub trait Persistable: Send + Sync {
    /// Flush to durable storage if dirty; no-op if clean.
    async fn persist(&self) -> DbResult<()>;
}

/// Registry of metadata blobs that need flushing at end-of-batch.
///
/// `flush_all()` calls `persist()` on each registered item in order.
/// Dirty ones flush; clean ones short-circuit at their own dirty-flag
/// check. Errors are collected: the first failure is returned after
/// attempting every remaining item (best-effort).
///
/// `Clone` shallow-copies the `Arc` handles — all clones share the
/// same underlying metadata blobs, which is what `TableManager::clone`
/// requires (clones of a manager must flush the same interner and
/// counter instances, not independent copies).
#[derive(Clone)]
pub struct PersistRegistry {
    items: Vec<Arc<dyn Persistable>>,
}

impl PersistRegistry {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Register a metadata blob to be flushed by `flush_all`.
    pub fn register(&mut self, item: Arc<dyn Persistable>) {
        self.items.push(item);
    }

    /// Flush all dirty items. The first error encountered is returned
    /// after attempting every remaining item.
    pub async fn flush_all(&self) -> DbResult<()> {
        let mut first_err = None;
        for item in &self.items {
            if let Err(e) = item.persist().await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Default for PersistRegistry {
    fn default() -> Self {
        Self::new()
    }
}
