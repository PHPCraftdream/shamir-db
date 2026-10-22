//! Repo-level tests for the durable per-(db,repo) follower replication
//! bookmark (`RepoInstance::replication_bookmark`), v3-only (PR6b-3):
//! the v3 projection (0 on fresh; the value written via `save_bookmark_v3`),
//! the typed refusal on legacy bytes, and durable reopen survival. Receipt
//! checkpointing owns advancement — there is no public advance API.
//!
//! The reopen test uses a disk-backed (fjall tempdir) repo so the persisted
//! record survives a drop+reopen exactly as a real process restart would
//! observe — an in-memory repo loses its stores on drop, so it cannot
//! prove durability.

#[cfg(feature = "fjall")]
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::RecordKey;

use crate::meta::repl::bookmark_v3::{load_bookmark_v3, save_bookmark_v3, BookmarkV3};
use crate::meta::MetaEnvelope;
use crate::repo::{BoxRepo, BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;

/// Build an in-memory repo with one configured table `items`.
fn mem_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("items")],
    )
}

fn bookmark_key() -> RecordKey {
    RecordKey::from_slice(
        crate::meta::MetaKey::ReplicationBookmark
            .as_record_id()
            .as_bytes(),
    )
}

async fn write_bytes(repo: &RepoInstance, bytes: Vec<u8>) {
    let info = repo.tx_info_store().await.unwrap();
    info.set(bookmark_key(), Bytes::from(bytes)).await.unwrap();
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

/// A fresh repo reports `replication_bookmark() == 0` (no v3 record yet).
#[tokio::test]
async fn default_bookmark_is_zero() {
    let repo = mem_repo();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 0);
}

// ── Test 2: v3 projection of a written record ───────────────────────

/// `save_bookmark_v3(5)` then read back 5 — the loader is a strict v3
/// projection, not a legacy read.
#[tokio::test]
async fn v3_projection_reads_the_written_record() {
    let repo = mem_repo();
    let info = repo.tx_info_store().await.unwrap();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: None,
            bookmark: 5,
            clean_through: Some(3),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.replication_bookmark().await.unwrap(), 5);
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.bookmark, 5);
    assert_eq!(v3.clean_through, Some(3));
}

// ── Test 3: refusal on legacy bytes ─────────────────────────────────

/// Legacy u64 and v2 bookmark bytes at the bookmark key are REFUSED with
/// the typed error — never treated as absent, never silently re-read.
#[tokio::test]
async fn legacy_bytes_refuse_the_projection() {
    use shamir_storage::error::{DbError, FollowerRefusalReason};
    let fence = crate::meta::repl_bookmark::FenceRecord {
        bookmark: 10,
        lineage_id: [5; 16],
        history_epoch: 3,
        observed_identity: None,
    };
    let legacy = vec![
        MetaEnvelope::new(10u64).encode().unwrap(),
        MetaEnvelope::new((2u16, (Some(fence), 10u64, Some(8u64))))
            .encode()
            .unwrap(),
    ];
    for bytes in legacy {
        let repo = mem_repo();
        write_bytes(&repo, bytes).await;
        let error = repo.replication_bookmark().await.unwrap_err();
        assert!(
            matches!(
                error,
                DbError::FollowerStateRefused {
                    reason: FollowerRefusalReason::LegacyBookmarkFormat
                }
            ),
            "expected the typed legacy refusal, got {error:?}"
        );
    }
}

// ── Test 4: survives drop+reopen (durable backend) ──────────────────

/// Write a v3 record with bookmark 7 on a disk-backed repo, drop the
/// `RepoInstance`, reopen a fresh instance over the SAME tempdir, and
/// verify the bookmark reads back as 7. Proves the record is durable
/// across a real restart (an in-memory repo loses its stores on drop and
/// cannot prove this).
#[cfg(feature = "fjall")]
#[tokio::test]
async fn bookmark_survives_reopen() {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    // Session 1: write the v3 record.
    {
        let repo = open_disk("r", path.clone(), vec![TableConfig::new("t")]).await;
        let _ = repo.get_table("t").await.unwrap();
        let info = repo.tx_info_store().await.unwrap();
        save_bookmark_v3(
            &info,
            &BookmarkV3 {
                fence: None,
                bookmark: 7,
                clean_through: Some(7),
                outstanding_receipt: None,
            },
        )
        .await
        .unwrap();
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
