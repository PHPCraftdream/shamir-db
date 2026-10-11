//! RepoInstance's lazy changefeed must restore marker metadata before reads.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Repo;
use shamir_tx::changefeed::{gap_marker_key, version_key};

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;

async fn instance_with_marker(marker: Option<Bytes>) -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    if let Some(marker) = marker {
        // Preload the actual backend, before constructing/using the lazy feed.
        repo.store_get("__changelog__")
            .await
            .unwrap()
            .set(gap_marker_key().into(), marker)
            .await
            .unwrap();
    }
    RepoInstance::new("gap-marker-test".into(), BoxRepo::InMemory(repo), vec![])
}

#[tokio::test]
async fn persisted_gap_is_restored_before_first_read() {
    let instance = instance_with_marker(Some(version_key(17))).await;
    let read = instance.read_changelog_from(1, 10).await.unwrap();
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(17));
}

#[tokio::test]
async fn fresh_repo_has_no_gap() {
    let instance = instance_with_marker(None).await;
    let read = instance.read_changelog_from(1, 10).await.unwrap();
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, None);
}

#[tokio::test]
async fn malformed_marker_initializes_with_conservative_gap() {
    let instance = instance_with_marker(Some(Bytes::from_static(&[1, 2, 3]))).await;
    let read = instance.read_changelog_from(1, 10).await.unwrap();
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(1));
}

#[tokio::test]
async fn zero_marker_initializes_with_conservative_gap() {
    let instance = instance_with_marker(Some(version_key(0))).await;
    let read = instance.read_changelog_from(1, 10).await.unwrap();
    assert!(read.events.is_empty());
    assert_eq!(read.gap_at, Some(1));
}
