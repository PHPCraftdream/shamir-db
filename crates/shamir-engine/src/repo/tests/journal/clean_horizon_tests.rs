//! Conservative cold certification over the same store after a hard drop.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use shamir_query_builder::write::doc;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::{Repo, Store};
use shamir_tx::changefeed::{
    clean_horizon_key, decode_clean_horizon, decode_gap_marker, encode_gap_marker, gap_marker_key,
};
use shamir_types::codecs::interned::query_value_to_inner;

use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;
use crate::tx::IsolationLevel;

fn open(backing: &Arc<InMemoryRepo>) -> RepoInstance {
    RepoInstance::new(
        "clean-test".into(),
        BoxRepo::InMemory(backing.clone()),
        vec![TableConfig::new("items")],
    )
}

async fn commit(repo: &RepoInstance) -> u64 {
    let table = repo.get_table("items").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let value =
        query_value_to_inner(&doc().set("name", "value").unwrap().build(), interner).unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    repo.tx_gate()
        .await
        .unwrap()
        .version_allocation_high_water_mark()
}

async fn j(store: &Arc<dyn Store>) -> u64 {
    decode_clean_horizon(&store.get(clean_horizon_key().into()).await.unwrap()).unwrap()
}

// Stop only the writer, NOT the clean-checkpoint path. Waiting for termination
// prevents the background drainer's retained feed from racing a new writer.
async fn stop_writer(repo: &RepoInstance) {
    repo.changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
}

// Red without clean J: a complete journal still receives the conservative gap.
#[tokio::test]
async fn clean_shutdown_hard_reopen_has_no_new_gap() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let h = commit(&repo).await;
    repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
    let store = backing.store_get("__changelog__").await.unwrap();
    assert_eq!(j(&store).await, h);
    stop_writer(&repo).await;
    drop(repo);
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let handle = repo.changefeed().await.unwrap();
    assert_eq!(
        handle.feed.read_from(&handle.store, 0, 100).await.gap_at,
        None,
    );
    assert_eq!(handle.feed.clean_horizon(), h);
    handle.feed.shutdown_and_wait().await.unwrap();
}

// Red without cold certification: complete events wrongly conceal unclean stop.
#[tokio::test]
async fn hard_drop_certifies_cursors_and_preserves_old_interval() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let first = commit(&repo).await;
    repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
    stop_writer(&repo).await;
    drop(repo);
    // New admission lifecycle: never commit on the stopped clean instance.
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let h = commit(&repo).await;
    repo.flush_buffers().await.unwrap();
    let handle = repo.changefeed().await.unwrap();
    handle.feed.record_recovery_gap(1, first).await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let handle = repo.changefeed().await.unwrap();
    let store = backing.store_get("__changelog__").await.unwrap();
    let interval = store.get(gap_marker_key().into()).await.unwrap();
    assert_eq!(decode_gap_marker(&interval).unwrap(), (1, h));
    for from in 0..=h {
        assert_eq!(
            handle.feed.read_from(&handle.store, from, 100).await.gap_at,
            Some(from.max(1)),
        );
    }
    assert_eq!(j(&store).await, h);
    commit(&repo).await;
    repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
    let pull = handle.feed.read_from(&handle.store, h + 1, 100).await;
    assert_eq!(pull.gap_at, None);
    assert_eq!(pull.events.len(), 1);
    handle.feed.shutdown_and_wait().await.unwrap();
}

// No J is a legacy directory. Reopening again does not widen the interval.
#[tokio::test]
async fn legacy_certifies_once_and_metadata_never_enters_ranges() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = open(&backing);
    let h = commit(&repo).await;
    repo.flush_buffers().await.unwrap();
    stop_writer(&repo).await;
    drop(repo);
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    let store = backing.store_get("__changelog__").await.unwrap();
    let gap = store.get(gap_marker_key().into()).await.unwrap();
    assert_eq!(decode_gap_marker(&gap).unwrap(), (1, h));
    repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
    stop_writer(&repo).await;
    drop(repo);
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    assert_eq!(store.get(gap_marker_key().into()).await.unwrap(), gap);
    let handle = repo.changefeed().await.unwrap();
    let rows = handle.store.range_from(Bytes::new(), 100).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        handle
            .feed
            .read_from(&handle.store, h + 1, 100)
            .await
            .gap_at,
        None,
    );
    handle.feed.shutdown_and_wait().await.unwrap();
}

// A staged repair with absent J models the durable state after failed J set.
#[tokio::test]
async fn gap_without_j_recertifies_idempotently_and_bad_j_refuses_recovery() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = open(&backing);
    let h = commit(&repo).await;
    repo.flush_buffers().await.unwrap();
    stop_writer(&repo).await;
    drop(repo);
    let store = backing.store_get("__changelog__").await.unwrap();
    store
        .set(gap_marker_key().into(), encode_gap_marker((1, h)))
        .await
        .unwrap();
    store.flush().await.unwrap();
    let repo = open(&backing);
    repo.recover_v2_inflight().await.unwrap();
    assert_eq!(j(&store).await, h);
    let interval = store.get(gap_marker_key().into()).await.unwrap();
    assert_eq!(decode_gap_marker(&interval).unwrap(), (1, h));
    stop_writer(&repo).await;
    drop(repo);
    store
        .set(clean_horizon_key().into(), Bytes::from_static(b"bad"))
        .await
        .unwrap();
    let repo = open(&backing);
    assert!(repo.recover_v2_inflight().await.is_err());
}
