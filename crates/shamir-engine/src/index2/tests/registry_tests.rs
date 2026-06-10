use crate::index2::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::kind::IndexKind;
use crate::index2::registry::IndexRegistry;
use async_trait::async_trait;
use shamir_storage::types::Store;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::sync::Arc;

struct DummyBackend(IndexDescriptor);

#[async_trait]
impl IndexBackend for DummyBackend {
    fn descriptor(&self) -> &IndexDescriptor {
        &self.0
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
