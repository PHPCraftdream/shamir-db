//! Lock-free index registry.
//!
//! Uses `scc::HashMap` (CAS-based) for both `id → backend` and
//! `name_interned → id` lookups. `next_id` is an `AtomicU32` —
//! `fetch_add(Relaxed)` is enough for unique-id generation since the
//! counter is single-source (no cross-process coordination).

use crate::backend::{IndexBackend, IndexError};
use shamir_collections::THasher;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct IndexRegistry {
    by_id: scc::HashMap<u32, Arc<dyn IndexBackend>, THasher>,
    by_name: scc::HashMap<u64, u32, THasher>,
    next_id: AtomicU32,
}

impl IndexRegistry {
    pub fn new() -> Self {
        Self {
            by_id: scc::HashMap::with_hasher(THasher::default()),
            by_name: scc::HashMap::with_hasher(THasher::default()),
            next_id: AtomicU32::new(1),
        }
    }

    /// Atomically allocate the next monotonic ID. Lock-free.
    pub fn allocate_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn insert(&self, backend: Arc<dyn IndexBackend>) -> Result<(), IndexError> {
        let d = backend.descriptor();
        let id = d.id;
        let name_interned = d.name_interned;

        self.by_id
            .insert_async(id, backend.clone())
            .await
            .map_err(|_| IndexError::Backend(format!("index id {id} already registered")))?;
        self.by_name
            .insert_async(name_interned, id)
            .await
            .map_err(|_| {
                IndexError::Backend(format!("index name {name_interned} already registered"))
            })?;
        Ok(())
    }

    pub async fn get_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        self.by_id.read_async(&id, |_, v| v.clone()).await
    }

    pub async fn get_by_name(&self, name_interned: u64) -> Option<Arc<dyn IndexBackend>> {
        let id = self.by_name.read_async(&name_interned, |_, v| *v).await?;
        self.get_by_id(id).await
    }

    pub async fn remove_by_id(&self, id: u32) -> Option<Arc<dyn IndexBackend>> {
        let removed = self.by_id.remove_async(&id).await.map(|(_, v)| v);
        if let Some(ref backend) = removed {
            let name_interned = backend.descriptor().name_interned;
            let _ = self.by_name.remove_async(&name_interned).await;
        }
        removed
    }

    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn peek_next_id(&self) -> u32 {
        self.next_id.load(Ordering::Relaxed)
    }

    pub fn set_next_id(&self, id: u32) {
        self.next_id.store(id, Ordering::Relaxed);
    }

    /// Collect all registered backends (snapshot).
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub async fn all_backends(&self) -> Vec<Arc<dyn IndexBackend>> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .iter_async(|_, v| {
                out.push(v.clone());
                true
            })
            .await;
        out
    }

    /// Collect all descriptors (for persistence).
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub async fn all_descriptors(&self) -> Vec<crate::descriptor::IndexDescriptor> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .iter_async(|_, v| {
                out.push(v.descriptor().clone());
                true
            })
            .await;
        out
    }

    /// Update the `by_name` mapping from `old_name_interned` to `new_name_interned`
    /// without touching the physical posting entries (they are keyed by `index_id`, not
    /// by name). Returns `true` if the entry was found and updated, `false` otherwise.
    ///
    /// This is the rekey primitive for RENAME INDEX on index2 backends: since
    /// posting keys embed the compact `u32` id (not the interned string id), the
    /// stored data survives a rename without any scan/copy.
    pub async fn rename_entry(&self, old_name_interned: u64, new_name_interned: u64) -> bool {
        // Look up the numeric id behind the old name.
        let id = match self.by_name.read_async(&old_name_interned, |_, v| *v).await {
            Some(v) => v,
            None => return false,
        };
        // Remove old name mapping.
        let _ = self.by_name.remove_async(&old_name_interned).await;
        // Insert new name mapping. If insertion fails (new_name already registered)
        // re-insert the old mapping to keep the registry consistent, then return false.
        if self
            .by_name
            .insert_async(new_name_interned, id)
            .await
            .is_err()
        {
            // Restore old entry on conflict.
            let _ = self.by_name.insert_async(old_name_interned, id).await;
            return false;
        }
        true
    }

    /// Find a backend whose first field path matches and whose kind
    /// matches the given tag ("fts", "functional", "vector").
    pub async fn find_by_field_and_kind(
        &self,
        field_path: &[u64],
        kind_tag: &str,
    ) -> Option<Arc<dyn IndexBackend>> {
        let mut found = None;
        self.by_id
            .iter_async(|_, backend| {
                let desc = backend.descriptor();
                let kind_matches = matches!(
                    (&desc.kind, kind_tag),
                    (crate::kind::IndexKind::Fts { .. }, "fts")
                        | (crate::kind::IndexKind::Functional(_), "functional")
                        | (crate::kind::IndexKind::Vector(_), "vector")
                        | (crate::kind::IndexKind::Btree { .. }, "btree")
                );
                if kind_matches && !desc.paths.is_empty() && desc.paths[0] == field_path {
                    found = Some(backend.clone());
                    return false;
                }
                true
            })
            .await;
        found
    }
}

impl Default for IndexRegistry {
    fn default() -> Self {
        Self::new()
    }
}
