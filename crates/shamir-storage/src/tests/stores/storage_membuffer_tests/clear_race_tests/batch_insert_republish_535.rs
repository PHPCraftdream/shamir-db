use super::super::common::no_flush_config;
use crate::membuffer_clear_race_hook::BatchInsertPauseHook;
use crate::storage_in_memory::InMemoryRepo;
use crate::storage_membuffer::MemBufferStore;
use crate::types::{Repo, Store};
use bytes::Bytes;
use std::sync::Arc;

// Drain the first batch item while the writer is parked, then check the second.
#[tokio::test]
async fn insert_many_second_item_survives_drain_racing_between_iterations() {
    let inner_repo = InMemoryRepo::new();
    let inner_store = inner_repo.store_get("t").await.unwrap();

    let buffered = Arc::new(MemBufferStore::new(inner_store, no_flush_config()));
    // Prevent the background flusher from racing the manually driven drain.
    buffered.disable_background_flusher_for_test();

    let hook = Arc::new(BatchInsertPauseHook::new());
    buffered.set_batch_pause_hook(Some(Arc::clone(&hook)));

    let buffered_writer = Arc::clone(&buffered);
    let insert_task = tokio::spawn(async move {
        buffered_writer
            .insert_many(vec![
                Bytes::from_static(b"v0-535"),
                Bytes::from_static(b"v1-535"),
            ])
            .await
    });

    hook.wait_until_parked().await;
    let drained = buffered.drain_once_for_test(usize::MAX).await.unwrap();
    assert_eq!(drained, 1, "the racing drain should have flushed only v0");
    assert!(
        !buffered.dirty_nonempty_flag(),
        "dirty count should be zero before v1 is published"
    );

    hook.release();
    let keys = insert_task
        .await
        .unwrap()
        .expect("insert_many must succeed");
    assert_eq!(keys.len(), 2);
    let v1_key = keys[1].clone();

    assert!(
        buffered.dirty_nonempty_flag(),
        "v1 must be counted after publication"
    );
    buffered.evict_from_cache_for_test(&v1_key).await;
    let got = buffered.get(v1_key).await.unwrap();
    assert_eq!(got.as_ref(), b"v1-535");

    let v0_key = keys[0].clone();
    assert!(
        buffered.get(v0_key).await.is_ok(),
        "v0 should have been flushed to inner by the racing drain"
    );
}
