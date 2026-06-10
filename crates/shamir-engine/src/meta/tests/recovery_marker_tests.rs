use crate::meta::recovery_marker::{
    load_last_committed, load_next_tx_id_snapshot, save_last_committed, save_next_tx_id_snapshot,
};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn load_missing_returns_none() {
    let s = mem_store();
    assert_eq!(load_last_committed(&s).await.unwrap(), None);
    assert_eq!(load_next_tx_id_snapshot(&s).await.unwrap(), None);
}

#[tokio::test]
async fn last_committed_round_trip() {
    let s = mem_store();
    save_last_committed(&s, 42).await.unwrap();
    assert_eq!(load_last_committed(&s).await.unwrap(), Some(42));
}

#[tokio::test]
async fn next_tx_id_round_trip() {
    let s = mem_store();
    save_next_tx_id_snapshot(&s, 1_234_567).await.unwrap();
    assert_eq!(load_next_tx_id_snapshot(&s).await.unwrap(), Some(1_234_567));
}

#[tokio::test]
async fn last_committed_overwrites() {
    let s = mem_store();
    save_last_committed(&s, 1).await.unwrap();
    save_last_committed(&s, 999).await.unwrap();
    assert_eq!(load_last_committed(&s).await.unwrap(), Some(999));
}

#[tokio::test]
async fn markers_dont_collide() {
    let s = mem_store();
    save_last_committed(&s, 10).await.unwrap();
    save_next_tx_id_snapshot(&s, 20).await.unwrap();
    assert_eq!(load_last_committed(&s).await.unwrap(), Some(10));
    assert_eq!(load_next_tx_id_snapshot(&s).await.unwrap(), Some(20));
}
