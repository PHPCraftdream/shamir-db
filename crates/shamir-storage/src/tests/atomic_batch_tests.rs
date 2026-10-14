//! Repository-wide atomic-batch tests (PR6b-1): in-memory + fjall backends.

use crate::error::DbError;
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
async fn atomic_batch_invalid_store_name_fails_at_staging() {
    let repo = InMemoryRepo::new();
    let mut batch = repo.begin_atomic_batch().await.unwrap();

    let k: RecordKey = Bytes::from_static(b"k1").into();
    batch
        .set("valid", k.clone(), Bytes::from_static(b"v1"))
        .unwrap();

    // Contract: staging-time validation is defense-in-depth, duplicated in
    // `exec_atomic_commit` before any write; an invalid store name is
    // rejected eagerly at `set` and never enters the batch.
    let err = batch
        .set(
            "",
            Bytes::from_static(b"k2").into(),
            Bytes::from_static(b"v2"),
        )
        .unwrap_err();
    assert!(
        matches!(err, DbError::Validation(_)),
        "expected Validation error at staging: {err:?}"
    );

    // The invalid op was never staged, so the batch holds ONLY the valid op
    // and commit succeeds.
    let outcome = batch.commit().await.unwrap();
    assert_eq!(outcome, AtomicCommitOutcome::Committed);

    // The valid op landed; the refused op has no partial state to observe
    // (an empty store name cannot be fetched).
    let store = repo.store_get("valid").await.unwrap();
    assert_eq!(store.get(k).await.unwrap().as_ref(), b"v1");
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

#[cfg(feature = "fjall")]
mod fjall {
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

    fn key(b: &'static [u8]) -> RecordKey {
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
        // No store handle may outlive `drop(repo)` across a `reopen`:
        // `FjallStore` holds an `Arc<Database>` clone that keeps the
        // fjall lock-file alive.
        drop(s1);
        drop(s2);
        drop(repo);

        // Second reopen: assert again.
        let repo = reopen(path).await;
        let s1 = repo.store_get("s1").await.unwrap();
        assert_eq!(s1.get(key(b"k1")).await.unwrap().as_ref(), b"v1");
        assert!(s1.get(key(b"k3")).await.is_err());
        let s2 = repo.store_get("s2").await.unwrap();
        assert_eq!(s2.get(key(b"k2")).await.unwrap().as_ref(), b"v2");
        drop(s1);
        drop(s2);
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
        // The handle must be released before the later `reopen`: it keeps an
        // `Arc<Database>` (and thus the fjall lock) alive.
        drop(s1);

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
        drop(s1);
        drop(s2);
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
        // Invariant: no store handle may outlive `drop(repo)` across a
        // `reopen`, because `FjallStore` holds an `Arc<Database>` clone that
        // keeps the fjall lock-file (and thus the lock) alive. This test
        // reopens before taking any store handles, so it complies.
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
}
