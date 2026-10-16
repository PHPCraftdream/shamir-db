//! Activation-routing tests (PR6b-2, Q1): `receipt_path_selected` must not
//! treat an absent bookmark record as proof of a fresh install — a durable
//! legacy recovery floor marker without a bookmark is LEGACY evidence, while
//! a v3 record routes to the receipt path.

#[cfg(feature = "fjall")]
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "fjall")]
use std::time::Duration;

use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};

use super::super::resume::activation::receipt_path_selected;
use crate::meta::recovery_marker::save_last_committed_flushed;
use crate::meta::repl::bookmark::save_bookmark_and_clean_horizon;
use crate::meta::repl::bookmark_v3::{save_bookmark_v3, BookmarkV3};
#[cfg(feature = "fjall")]
use crate::repo::BoxRepoFactory;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::TableConfig;

fn repo() -> RepoInstance {
    RepoInstance::new(
        "follower-activation".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("items")],
    )
}

fn info() -> Arc<dyn shamir_storage::types::Store> {
    Arc::new(InMemoryStore::new())
}

/// Fresh install: no marker, no bookmark → receipt path (InMemory supports
/// repository-wide atomic batches, so no `AtomicBatchUnsupported` error).
#[tokio::test]
async fn empty_store_selects_receipt_path() {
    let repo = repo();
    let info = info();
    assert!(repo.storage_supports_atomic_batch());
    assert!(receipt_path_selected(&repo, &info).await.unwrap());
}

/// Legacy evidence: durable floor marker without any bookmark → stay legacy.
#[tokio::test]
async fn marker_without_bookmark_selects_legacy_path() {
    let repo = repo();
    let info = info();
    save_last_committed_flushed(&info, 2).await.unwrap();
    assert!(!receipt_path_selected(&repo, &info).await.unwrap());
}

/// Legacy v2 bookmark record → legacy path.
#[tokio::test]
async fn legacy_v2_record_selects_legacy_path() {
    let repo = repo();
    let info = info();
    save_bookmark_and_clean_horizon(&info, 2, 1).await.unwrap();
    assert!(!receipt_path_selected(&repo, &info).await.unwrap());
}

/// v3 bookmark record → receipt path even alongside the usual floor marker.
#[tokio::test]
async fn v3_record_with_floor_marker_selects_receipt_path() {
    let repo = repo();
    let info = info();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: None,
            bookmark: 2,
            clean_through: Some(2),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    save_last_committed_flushed(&info, 2).await.unwrap();
    assert!(receipt_path_selected(&repo, &info).await.unwrap());
}

#[cfg(feature = "fjall")]
async fn open_with_retry(path: &Path, name: &str) -> RepoInstance {
    // Match the crash harness: fjall's OS-level Database lock can linger after close.
    for attempt in 0..25u32 {
        match RepoInstance::from_factory(
            name.to_owned(),
            BoxRepoFactory::fjall_raw(path),
            vec![TableConfig::new("items")],
        )
        .await
        {
            Ok(repo) => return repo,
            Err(error) if attempt < 24 => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("reopen failed after retries: {error}"),
        }
    }
    unreachable!("retry loop always returns or panics")
}

#[cfg(feature = "fjall")]
#[tokio::test]
async fn durable_marker_without_bookmark_selects_legacy_after_reopen() {
    // No existing legacy seam aborts between floor reservation and the first bookmark; seed that durable state directly.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_path_buf();
    let name = "activation-restart";
    let first = open_with_retry(&path, name).await;
    let first_info = first.raw_store("__tx__").await.unwrap();
    save_last_committed_flushed(&first_info, 2).await.unwrap();
    drop(first_info);
    let handle = first.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(first);

    let reopened = open_with_retry(&path, name).await;
    let reopened_info = reopened.raw_store("__tx__").await.unwrap();
    assert!(!receipt_path_selected(&reopened, &reopened_info)
        .await
        .unwrap());
    drop(reopened_info);
    let handle = reopened.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(reopened);
}

#[tokio::test]
async fn v3_record_without_floor_marker_selects_receipt_path() {
    let repo = repo();
    let info = info();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: None,
            bookmark: 2,
            clean_through: Some(2),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert!(receipt_path_selected(&repo, &info).await.unwrap());
}
