//! Disk hard reopen with runtime teardown (no sleeps / file-lock retries).
#![cfg(feature = "fjall")]

use std::time::Duration;

use bytes::Bytes;
use shamir_query_builder::write::doc;
use shamir_types::codecs::interned::query_value_to_inner;
use tokio::runtime::Builder;

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::{BoxRepoFactory, RepoConfig, RepoInstance};
use crate::engine::table::TableConfig;
use crate::engine::tx::IsolationLevel;
use crate::storage::storage_fjall::FjallRepo;
use crate::storage::types::Repo;

// Red if registry insertion precedes certification: Err leaves a reachable repo.
#[test]
fn corrupt_j_refuses_add_and_config_open_without_registry_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let backing = FjallRepo::new(&path).unwrap();
        let store = backing.store_get("__changelog__").await.unwrap();
        store
            .set(
                Bytes::from_static(b"\0journal-clean-v1").into(),
                Bytes::from_static(b"bad"),
            )
            .await
            .unwrap();
        store.flush().await.unwrap();
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = DbInstance::new();
        assert!(db
            .add_repo(RepoConfig::new("repo", BoxRepoFactory::fjall_raw(&path)))
            .await
            .is_err());
        assert!(db.get_repo("repo").is_none());
        assert!(db.list_repos().is_empty());
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let config = RepoConfig::new("repo", BoxRepoFactory::fjall_raw(&path));
        assert!(DbInstance::with_repos(vec![config]).await.is_err());
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        assert!(RepoInstance::from_factory(
            "repo".into(),
            BoxRepoFactory::fjall_raw(&path),
            vec![],
        )
        .await
        .is_err());
    });
    drop(runtime);
}

// Red before cold certification: hard stop yields no gap even with full events.
#[test]
fn hard_reopen_certifies_before_registry_and_clean_reopen_does_not_widen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("repo");
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let h = runtime.block_on(async {
        let db = DbInstance::new();
        db.add_repo(
            RepoConfig::new("repo", BoxRepoFactory::fjall_raw(&path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
        let repo = db.get_repo("repo").unwrap();
        let table = repo.get_table("items").await.unwrap();
        let interner = table.interner().get().await.unwrap();
        let value = query_value_to_inner(&doc().set("name", "value").build(), interner).unwrap();
        let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        table.insert_tx(&value, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(snapshot);
        repo.flush_buffers().await.unwrap();
        repo.current_commit_version().await.unwrap()
    });
    // Teardown cancels background owners; no clean shutdown was called.
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let db = DbInstance::new();
        db.add_repo(
            RepoConfig::new("repo", BoxRepoFactory::fjall_raw(&path))
                .add_table(TableConfig::new("items")),
        )
        .await
        .unwrap();
        let repo = db.get_repo("repo").unwrap();
        for from in 0..=h {
            let journal = repo.read_changelog_from(from, 100).await.unwrap();
            assert_eq!(journal.gap_at, None);
            assert_eq!(journal.events.len(), 1);
            assert_eq!(journal.events[0].event.commit_version, h);
        }
        assert_eq!(
            repo.read_changelog_from(h + 1, 100).await.unwrap().gap_at,
            None,
        );
        repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
    });
    drop(runtime);
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let config = RepoConfig::new("repo", BoxRepoFactory::fjall_raw(&path))
            .add_table(TableConfig::new("items"));
        let db = DbInstance::with_repos(vec![config]).await.unwrap();
        let repo = db.get_repo("repo").unwrap();
        assert_eq!(
            repo.read_changelog_from(h + 1, 100).await.unwrap().gap_at,
            None,
        );
    });
    drop(runtime);
}
