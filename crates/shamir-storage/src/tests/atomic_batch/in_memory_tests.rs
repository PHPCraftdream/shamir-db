use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryRepo;
use crate::types::{AtomicCommitOutcome, RecordKey, Repo, Store};
use bytes::Bytes;

#[tokio::test]
async fn atomic_batch_stages_and_commits_across_stores() {
    let repo = InMemoryRepo::new();
    let mut batch = repo.begin_atomic_batch().await.unwrap();

    let ka: RecordKey = Bytes::from_static(b"a1").into();
    let kb: RecordKey = Bytes::from_static(b"b1").into();
    let kd: RecordKey = Bytes::from_static(b"b2").into();
    batch
        .set("a", ka.clone(), Bytes::from_static(b"va1"))
        .unwrap();
    batch
        .set("b", kb.clone(), Bytes::from_static(b"vb1"))
        .unwrap();
    batch.delete("b", kd.clone()).unwrap();

    let outcome = batch.commit().await.unwrap();
    assert_eq!(outcome, AtomicCommitOutcome::Committed);

    let store_a = repo.store_get("a").await.unwrap();
    assert_eq!(store_a.get(ka).await.unwrap().as_ref(), b"va1");

    let store_b = repo.store_get("b").await.unwrap();
    assert_eq!(store_b.get(kb).await.unwrap().as_ref(), b"vb1");
    // The staged delete is visible as removed.
    assert!(store_b.get(kd).await.is_err());
}

#[tokio::test]
async fn atomic_batch_invalid_store_name_is_all_or_nothing() {
    let repo = InMemoryRepo::new();
    let mut batch = repo.begin_atomic_batch().await.unwrap();

    let k: RecordKey = Bytes::from_static(b"k1").into();
    batch
        .set("valid", k.clone(), Bytes::from_static(b"v1"))
        .unwrap();
    batch
        .set(
            "",
            Bytes::from_static(b"k2").into(),
            Bytes::from_static(b"v2"),
        )
        .unwrap();

    assert!(batch.commit().await.is_err());

    // Nothing visible on the valid store staged alongside.
    let store = repo.store_get("valid").await.unwrap();
    assert!(store.get(k).await.is_err());
}

#[tokio::test]
async fn atomic_batch_capability_flags() {
    let repo = InMemoryRepo::new();
    assert!(repo.supports_atomic_batch());

    // Wrapper backends inherit the trait default `false`: the repo-wide
    // batch writes through the RAW backend by store name, bypassing any
    // wrapper cache, so wrapper stacks must report unsupported.
    let inner = std::sync::Arc::new(crate::storage_in_memory::InMemoryStore::new());
    let cached = CachedStore::new_sync(inner).await.unwrap();
    assert!(!cached.supports_atomic_batch());
}
