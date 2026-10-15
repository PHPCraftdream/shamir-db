//! Counter persistence must admit before sampling stale cache state.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::codecs::basic::bincode;

use crate::meta::MetaKey;
use crate::repo::RepoWriteDomain;
use crate::table::record_counter::RecordCounter;

fn count_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::Count.as_record_id().as_bytes())
}

async fn durable_count(info: &Arc<dyn Store>) -> Option<u64> {
    match info.get(count_key()).await {
        Ok(bytes) => Some(bincode::from_bytes(&bytes).expect("decode count")),
        Err(_) => None,
    }
}

fn counter_with(info: &Arc<dyn Store>, domain: &Arc<RepoWriteDomain>) -> RecordCounter {
    RecordCounter::new_with_domain(Arc::clone(info), Arc::clone(domain))
}

#[tokio::test]
async fn set_blocks_while_permit_held_then_writes_durable_value() {
    let domain = RepoWriteDomain::new("counter-set");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);
    let permit = domain.admit().await.expect("admit");
    let mut mutation = Box::pin(counter.set(7));
    assert!(futures::poll!(mutation.as_mut()).is_pending());
    assert_eq!(durable_count(&info).await, None);
    drop(permit);
    mutation.await.expect("set after release");
    assert_eq!(durable_count(&info).await, Some(7));
    assert_eq!(counter.get().await.unwrap(), 7);
}

#[tokio::test]
async fn set_to_and_persist_block_while_permit_held() {
    let domain = RepoWriteDomain::new("counter-set-to-persist");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);
    counter.increment(3).await.unwrap();
    counter.set(3).await.unwrap();
    let permit = domain.admit().await.expect("admit");
    let mut set_to = Box::pin(counter.set_to(42));
    assert!(futures::poll!(set_to.as_mut()).is_pending());
    assert_eq!(durable_count(&info).await, Some(3));
    let mut persist = Box::pin(counter.persist());
    assert!(futures::poll!(persist.as_mut()).is_pending());
    assert_eq!(counter.get().await.unwrap(), 3);
    drop(permit);
    set_to.await.expect("set_to after release");
    persist.await.expect("persist after release");
    assert_eq!(durable_count(&info).await, Some(42));
    assert_eq!(counter.get().await.unwrap(), 42);
}
#[tokio::test]
async fn stale_cache_persist_cannot_overwrite_new_absolute_count() {
    let domain = RepoWriteDomain::new("counter-stale-cache");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);
    counter.increment(7).await.expect("seed stale cache");
    assert_eq!(counter.get().await.unwrap(), 7);

    let permit = domain.admit().await.expect("hold admission");
    info.set(
        count_key(),
        bincode::to_bytes(&42_u64).expect("encode absolute"),
    )
    .await
    .expect("durable absolute count");
    assert_eq!(durable_count(&info).await, Some(42));

    let mut persistence = Box::pin(counter.persist());
    assert!(futures::poll!(persistence.as_mut()).is_pending());
    assert_eq!(durable_count(&info).await, Some(42));
    assert_eq!(counter.get().await.unwrap(), 7);

    counter
        .set_to_admitted(42, &permit)
        .await
        .expect("adopt absolute count");
    assert_eq!(counter.get().await.unwrap(), 42);
    assert_eq!(durable_count(&info).await, Some(42));
    drop(permit);
    persistence.await.expect("persist after release");
    assert_eq!(durable_count(&info).await, Some(42));
}

#[tokio::test]
async fn unprotected_stale_cache_persistence_negative_control_overwrites() {
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = RecordCounter::new(Arc::clone(&info));
    counter.increment(7).await.expect("seed stale cache");
    assert_eq!(counter.get().await.unwrap(), 7);
    info.set(
        count_key(),
        bincode::to_bytes(&42_u64).expect("encode absolute"),
    )
    .await
    .expect("durable absolute count");
    counter
        .persist_unprotected_for_test()
        .await
        .expect("unprotected persist");
    assert_eq!(durable_count(&info).await, Some(7));
}

#[tokio::test]
async fn increment_gains_no_admission() {
    let domain = RepoWriteDomain::new("counter-increment");
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counter = counter_with(&info, &domain);
    let _permit = domain.admit().await.expect("admit");
    counter.increment(5).await.unwrap();
    assert_eq!(counter.get().await.unwrap(), 5);
}
