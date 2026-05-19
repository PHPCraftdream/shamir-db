//! Lock-free index registry.
//!
//! Uses `scc::HashMap` (CAS-based) for both `id → backend` and
//! `name_interned → id` lookups. `next_id` is an `AtomicU32` —
//! `fetch_add(Relaxed)` is enough for unique-id generation since the
//! counter is single-source (no cross-process coordination).

use crate::index2::backend::{IndexBackend, IndexError};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub struct IndexRegistry {
    by_id: scc::HashMap<u32, Arc<dyn IndexBackend>>,
    by_name: scc::HashMap<u64, u32>,
    next_id: AtomicU32,
}

impl IndexRegistry {
    pub fn new() -> Self {
        Self {
            by_id: scc::HashMap::new(),
            by_name: scc::HashMap::new(),
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
            .map_err(|_| IndexError::Backend(format!("index name {name_interned} already registered")))?;
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
    pub async fn all_backends(&self) -> Vec<Arc<dyn IndexBackend>> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id.scan_async(|_, v| out.push(v.clone())).await;
        out
    }

    /// Collect all descriptors (for persistence).
    pub async fn all_descriptors(&self) -> Vec<crate::index2::descriptor::IndexDescriptor> {
        let mut out = Vec::with_capacity(self.by_id.len());
        self.by_id
            .scan_async(|_, v| out.push(v.descriptor().clone()))
            .await;
        out
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
            .scan_async(|_, backend| {
                if found.is_some() {
                    return;
                }
                let desc = backend.descriptor();
                let kind_matches = match (&desc.kind, kind_tag) {
                    (crate::index2::kind::IndexKind::Fts { .. }, "fts") => true,
                    (crate::index2::kind::IndexKind::Functional(_), "functional") => true,
                    (crate::index2::kind::IndexKind::Vector(_), "vector") => true,
                    (crate::index2::kind::IndexKind::Btree { .. }, "btree") => true,
                    _ => false,
                };
                if kind_matches && !desc.paths.is_empty() && desc.paths[0] == field_path {
                    found = Some(backend.clone());
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::backend::{IndexQuery, IndexResult};
    use crate::index2::descriptor::IndexDescriptor;
    use crate::index2::kind::IndexKind;
    use async_trait::async_trait;
    use shamir_storage::types::Store;
    use shamir_types::types::record_id::RecordId;
    use shamir_types::types::value::InnerValue;
    use smallvec::SmallVec;
    use std::collections::BTreeSet;

    struct DummyBackend(IndexDescriptor);

    #[async_trait]
    impl IndexBackend for DummyBackend {
        fn descriptor(&self) -> &IndexDescriptor {
            &self.0
        }
        async fn on_insert(&self, _: RecordId, _: &InnerValue) -> Result<(), IndexError> {
            Ok(())
        }
        async fn on_update(
            &self,
            _: RecordId,
            _: &InnerValue,
            _: &InnerValue,
        ) -> Result<(), IndexError> {
            Ok(())
        }
        async fn on_delete(&self, _: RecordId, _: &InnerValue) -> Result<(), IndexError> {
            Ok(())
        }
        async fn on_batch_insert(
            &self,
            _: &[(RecordId, &InnerValue)],
        ) -> Result<(), IndexError> {
            Ok(())
        }
        async fn lookup(&self, _: IndexQuery) -> Result<IndexResult, IndexError> {
            Ok(IndexResult::Set(BTreeSet::new()))
        }
        async fn rebuild(&self, _: Arc<dyn Store>) -> Result<(), IndexError> {
            Ok(())
        }
        async fn drop_all(&self) -> Result<(), IndexError> {
            Ok(())
        }
    }

    fn make(id: u32, name_interned: u64) -> Arc<dyn IndexBackend> {
        Arc::new(DummyBackend(IndexDescriptor::new(
            id,
            format!("idx_{id}"),
            name_interned,
            SmallVec::new(),
            IndexKind::Btree { unique: false },
        )))
    }

    #[tokio::test]
    async fn insert_and_lookup_by_id() {
        let reg = IndexRegistry::new();
        let b = make(10, 100);
        reg.insert(b).await.unwrap();
        let got = reg.get_by_id(10).await.unwrap();
        assert_eq!(got.descriptor().id, 10);
    }

    #[tokio::test]
    async fn insert_and_lookup_by_name() {
        let reg = IndexRegistry::new();
        reg.insert(make(11, 200)).await.unwrap();
        let got = reg.get_by_name(200).await.unwrap();
        assert_eq!(got.descriptor().id, 11);
    }

    #[tokio::test]
    async fn allocate_id_monotonic() {
        let reg = IndexRegistry::new();
        let a = reg.allocate_id();
        let b = reg.allocate_id();
        let c = reg.allocate_id();
        assert!(a < b && b < c);
    }

    #[tokio::test]
    async fn duplicate_id_rejected() {
        let reg = IndexRegistry::new();
        reg.insert(make(42, 1)).await.unwrap();
        let err = reg.insert(make(42, 2)).await.unwrap_err();
        assert!(matches!(err, IndexError::Backend(_)));
    }

    #[tokio::test]
    async fn remove_drops_both_maps() {
        let reg = IndexRegistry::new();
        reg.insert(make(7, 300)).await.unwrap();
        assert!(reg.get_by_name(300).await.is_some());
        let removed = reg.remove_by_id(7).await.unwrap();
        assert_eq!(removed.descriptor().id, 7);
        assert!(reg.get_by_id(7).await.is_none());
        assert!(reg.get_by_name(300).await.is_none());
    }
}
