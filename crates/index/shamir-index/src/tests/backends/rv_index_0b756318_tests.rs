use crate::backends::backend::IndexBackend;
use crate::descriptor::IndexDescriptor;
use crate::expr::IndexExpr;
use crate::functional_backend::FunctionalBackend;
use crate::kind::{FunctionalConfig, IndexKind};
use crate::tests::backends::remove_fail_store::RemoveFailStore;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, TouchInd};
use smallvec::SmallVec;
use std::sync::Arc;

#[tokio::test]
async fn rv_index_0b756318_functional_drop_propagates_remove_failure() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Seed at least one key under the drop prefix (id 919, raw le-bytes)
    // so `scan_prefix_stream` actually yields work and the injected remove
    // failure genuinely fires instead of trivially passing on an empty scan.
    let id: u32 = 919;
    let mut seed_key = id.to_le_bytes().to_vec();
    seed_key.push(b'x');
    inner
        .set(Bytes::from(seed_key).into(), Bytes::from_static(b"junk"))
        .await
        .unwrap();
    let store = Arc::new(RemoveFailStore::new(inner));
    let i = Interner::new();
    let name = match i.touch_ind("f").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    };
    let desc = IndexDescriptor::new(
        919,
        "f",
        name,
        SmallVec::new(),
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: IndexExpr::Field(vec![name]),
        })),
    );
    let backend = FunctionalBackend::new(desc, IndexExpr::Field(vec![name]), store);
    let result = backend.drop_all().await;
    assert!(result.is_err(), "remove failure must not be swallowed");
}
