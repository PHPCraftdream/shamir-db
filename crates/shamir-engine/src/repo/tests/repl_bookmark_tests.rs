//! R1-b — repo-level tests for the durable per-(db,repo) follower
//! replication bookmark (`RepoInstance::replication_bookmark` /
//! `advance_replication_bookmark`).
//!
//! These exercise the repo-level WRAPPERS (default-0, monotonic advance
//! rejection, durable reopen survival), complementing the codec-level
//! round-trip tests in `crate::meta::tests::repl_bookmark_tests`. The
//! reopen test uses a disk-backed (fjall tempdir) repo so the persisted
//! marker survives a drop+reopen exactly as a real process restart would
//! observe — an in-memory repo loses its stores on drop, so it cannot
//! prove durability.

use std::path::PathBuf;

use shamir_storage::storage_in_memory::InMemoryRepo;
use std::sync::Arc;

use crate::repo::{BoxRepo, BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;

/// Build an in-memory follower repo with one configured table `items`.
/// Used by the non-durability tests (default-0, advance+read, monotonicity)
/// — these do not need to survive a drop+reopen.
fn mem_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("items")],
    )
}

/// Open (or reopen) a disk-backed repo at `path`, retrying on Windows
/// where the backend releases its file lock lazily after `drop`. Mirrors
/// the helpers in `recovery_gate_tests` / `recovery_tests`.
#[cfg(feature = "fjall")]
async fn open_disk(name: &str, path: PathBuf, tables: Vec<TableConfig>) -> RepoInstance {
    let mut last_err = None;
    for _attempt in 0..10 {
        match RepoInstance::from_factory(
            name.into(),
            BoxRepoFactory::fjall_raw(path.clone()),
            tables.clone(),
        )
        .await
        {
            Ok(r) => return r,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!("open_disk({name:?}) failed after 10 retries: {last_err:?}");
}

// ── Test 1: default 0 on a fresh repo ───────────────────────────────

/// A fresh in-memory repo reports `replication_bookmark() == 0` (the
/// marker has never been written).
#[tokio::test]
async fn default_bookmark_is_zero() {
    let repo = mem_repo();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
}

// ── Test 2: advance + read ──────────────────────────────────────────

/// Advance to 5 then read back 5.
#[tokio::test]
async fn advance_then_read() {
    let repo = mem_repo();
    repo.advance_replication_bookmark(5).await.unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 5);
}

// ── Test 3: monotonicity (rollback rejection) ───────────────────────

/// Advance to 5, then attempt to advance to 3 (out-of-order delivery):
/// the bookmark must NOT roll back — it stays at 5. A subsequent advance
/// to 8 (strictly greater) does take effect.
#[tokio::test]
async fn monotonic_advance_rejects_rollback() {
    let repo = mem_repo();

    repo.advance_replication_bookmark(5).await.unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 5);

    // Out-of-order (stale) delivery: bookmark stays at 5.
    repo.advance_replication_bookmark(3).await.unwrap();
    assert_eq!(
        repo.replication_bookmark().await.unwrap(),
        5,
        "bookmark must not roll back from 5 to 3"
    );

    // Strictly-greater advance takes effect.
    repo.advance_replication_bookmark(8).await.unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 8);
}

// ── Test 3b: equal-value advance is a no-op (idempotent re-advance) ─

/// Advancing to the SAME value (e.g. re-delivery of the watermark itself)
/// is a no-op — the bookmark stays at the current value and no error is
/// returned. Guards against a `version > current` strictness surprise.
#[tokio::test]
async fn equal_advance_is_noop() {
    let repo = mem_repo();
    repo.advance_replication_bookmark(7).await.unwrap();
    repo.advance_replication_bookmark(7).await.unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 7);
}

// ── Test 4: survives drop+reopen (durable backend) ──────────────────

/// Advance to 7 on a disk-backed repo, drop the `RepoInstance`, reopen a
/// fresh instance over the SAME tempdir, and verify the bookmark reads
/// back as 7. Proves the marker is durable across a real restart (an
/// in-memory repo loses its stores on drop and cannot prove this).
#[cfg(feature = "fjall")]
#[tokio::test]
async fn bookmark_survives_reopen() {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // Session 1: advance to 7.
    {
        let repo = open_disk("r", path.clone(), vec![TableConfig::new("t")]).await;
        // Touch the table so the repo is fully instantiated.
        let _ = repo.get_table("t").await.unwrap();
        repo.advance_replication_bookmark(7).await.unwrap();
        assert_eq!(repo.replication_bookmark().await.unwrap(), 7);
    } // repo dropped here

    // Session 2: reopen over the same tempdir.
    let repo2 = open_disk("r", path.clone(), vec![TableConfig::new("t")]).await;
    let _ = repo2.get_table("t").await.unwrap();
    assert_eq!(
        repo2.replication_bookmark().await.unwrap(),
        7,
        "bookmark must survive drop+reopen on a durable backend"
    );
}
