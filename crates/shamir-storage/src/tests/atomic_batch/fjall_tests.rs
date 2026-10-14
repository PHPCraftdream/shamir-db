use crate::storage_fjall::FjallRepo;
use crate::types::{AtomicCommitOutcome, RecordKey, Repo};
use bytes::Bytes;
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

/// Reopen the repo with a bounded retry loop (the fjall file lock releases a
/// few ms after the previous handle drops). This is the sanctioned sibling
/// pattern from `storage_fjall_tests.rs`, not a condition-poll.
async fn reopen(path: &str) -> FjallRepo {
    for _ in 0..50 {
        match FjallRepo::new(path) {
            Ok(repo) => return repo,
            Err(_) => sleep(Duration::from_millis(100)).await,
        }
    }
    panic!("could not reopen fjall repo at {path} after 50 attempts");
}

fn key(b: &[u8]) -> RecordKey {
    Bytes::from_static(b).into()
}

#[tokio::test]
async fn atomic_batch_persists_across_reopen() {
    let path = "./test_data/atomic_batch_persists";
    if std::path::Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }

    let repo = FjallRepo::new(path).unwrap();
    let mut batch = repo.begin_atomic_batch().await.unwrap();
    batch
        .set("s1", key(b"k1"), Bytes::from_static(b"v1"))
        .unwrap();
    batch
        .set("s2", key(b"k2"), Bytes::from_static(b"v2"))
        .unwrap();
    batch.delete("s1", key(b"k3")).unwrap();
    let outcome = batch.commit().await.unwrap();
    assert_eq!(outcome, AtomicCommitOutcome::Committed);
    drop(repo);

    // First reopen: all three effects must have survived.
    let repo = reopen(path).await;
    let s1 = repo.store_get("s1").await.unwrap();
    assert_eq!(s1.get(key(b"k1")).await.unwrap().as_ref(), b"v1");
    assert!(s1.get(key(b"k3")).await.is_err());
    let s2 = repo.store_get("s2").await.unwrap();
    assert_eq!(s2.get(key(b"k2")).await.unwrap().as_ref(), b"v2");
    drop(repo);

    // Second reopen: assert again.
    let repo = reopen(path).await;
    let s1 = repo.store_get("s1").await.unwrap();
    assert_eq!(s1.get(key(b"k1")).await.unwrap().as_ref(), b"v1");
    assert!(s1.get(key(b"k3")).await.is_err());
    let s2 = repo.store_get("s2").await.unwrap();
    assert_eq!(s2.get(key(b"k2")).await.unwrap().as_ref(), b"v2");
    drop(repo);

    fs::remove_dir_all(path).ok();
}

/// Drop-guard reset so the injected-failure flags never leak into other
/// tests, even on assertion failure (panic unwinds through Drop).
struct SeamGuard;

impl Drop for SeamGuard {
    fn drop(&mut self) {
        crate::storage_fjall::atomic_batch_test_seam::reset();
    }
}

#[tokio::test]
async fn atomic_batch_all_or_nothing_on_injected_failure() {
    let path = "./test_data/atomic_batch_fail_before";
    if std::path::Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }
    let _seam = SeamGuard;

    let repo = FjallRepo::new(path).unwrap();
    let s1 = repo.store_get("s1").await.unwrap();
    s1.set(key(b"k"), Bytes::from_static(b"old")).await.unwrap();

    let mut batch = repo.begin_atomic_batch().await.unwrap();
    batch
        .set("s1", key(b"k"), Bytes::from_static(b"new"))
        .unwrap();
    batch
        .set("s2", key(b"k"), Bytes::from_static(b"x"))
        .unwrap();

    crate::storage_fjall::atomic_batch_test_seam::FAIL_BEFORE_COMMIT
        .store(true, std::sync::atomic::Ordering::Release);

    assert!(batch.commit().await.is_err());
    drop(repo);

    let repo = reopen(path).await;
    let s1 = repo.store_get("s1").await.unwrap();
    assert_eq!(s1.get(key(b"k")).await.unwrap().as_ref(), b"old");
    let s2 = repo.store_get("s2").await.unwrap();
    assert!(s2.get(key(b"k")).await.is_err());
    drop(repo);

    fs::remove_dir_all(path).ok();
}

#[tokio::test]
async fn atomic_batch_uncertain_outcome_is_typed() {
    let path = "./test_data/atomic_batch_uncertain";
    if std::path::Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }
    let _seam = SeamGuard;

    let repo = FjallRepo::new(path).unwrap();
    let mut batch = repo.begin_atomic_batch().await.unwrap();
    batch
        .set("u1", key(b"k"), Bytes::from_static(b"v"))
        .unwrap();
    batch
        .set("u2", key(b"k"), Bytes::from_static(b"w"))
        .unwrap();

    crate::storage_fjall::atomic_batch_test_seam::FAIL_AFTER_COMMIT
        .store(true, std::sync::atomic::Ordering::Release);

    let outcome = batch.commit().await.unwrap();
    match &outcome {
        AtomicCommitOutcome::Uncertain { .. } => {}
        AtomicCommitOutcome::Committed => panic!("expected Uncertain, got Committed"),
    }

    drop(repo);
    let repo = reopen(path).await;
    // Deterministic: the batch committed before the injected failure, so
    // both writes ARE visible after reopen.
    let u1 = repo.store_get("u1").await.unwrap();
    assert_eq!(u1.get(key(b"k")).await.unwrap().as_ref(), b"v");
    let u2 = repo.store_get("u2").await.unwrap();
    assert_eq!(u2.get(key(b"k")).await.unwrap().as_ref(), b"w");
    drop(repo);

    fs::remove_dir_all(path).ok();
}

/// The DEFAULT `Repo::begin_atomic_batch` (returning typed
/// `DbError::AtomicBatchUnsupported`) is reachable on any `Repo`
/// implementation that does not override it. Rather than author a throwaway
/// trait impl, this documents the contract and asserts `FjallRepo` reports
/// `true` — i.e. `FjallRepo` overrides BOTH `supports_atomic_batch` and
/// `begin_atomic_batch`, and the typed default-unsupported error exists on
/// the `Repo` trait for every backend that does not (e.g. wrapper stacks via
/// `supports_atomic_batch() == false`).
#[tokio::test]
async fn fjall_repo_reports_atomic_batch_support() {
    let path = "./test_data/atomic_batch_support_flag";
    if std::path::Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }
    let repo = FjallRepo::new(path).unwrap();
    assert!(repo.supports_atomic_batch());
    drop(repo);
    fs::remove_dir_all(path).ok();
}
