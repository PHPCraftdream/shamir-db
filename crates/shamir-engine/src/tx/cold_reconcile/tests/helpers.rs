use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::meta::recovery_marker::save_last_committed_flushed;
use crate::meta::repl_bookmark::{save_bookmark_and_clean_horizon, save_replication_bookmark};
use crate::repo::{BoxRepoFactory, RepoFactory, RepoInstance};
use crate::table::TableConfig;
use shamir_storage::error::DbResult;
use shamir_storage::types::{Repo as _, Store};
use shamir_tx::changefeed::cold::{encode_event, read_event};
use shamir_tx::changefeed::{clean_horizon_key, ChangelogEvent};
use shamir_wal::{WalDurability, WalEntryV2};

pub(super) const REPO_NAME: &str = "cold_reconcile_test";

pub(super) async fn open_unrecovered(path: &Path) -> RepoInstance {
    let repo = RepoInstance::from_factory_for_recovery_test(
        REPO_NAME.into(),
        BoxRepoFactory::fjall_raw(path.to_path_buf()),
        vec![TableConfig::new("items")],
    )
    .await
    .expect("open raw Fjall repo");
    repo.disable_background_drainer_for_test();
    repo
}

pub(super) async fn prepare_image(
    path: &Path,
    entries: &[WalEntryV2],
    journal_events: &[ChangelogEvent],
    bookmark: Option<u64>,
    clean_horizon: u64,
) {
    persist_image(path, entries, journal_events, bookmark, None, clean_horizon).await;
}

/// Follower fixture image: no surviving WAL entries, an upstream bookmark,
/// and the PR6 record that may carry a known local journal horizon.
pub(super) async fn prepare_follower_image(
    path: &Path,
    journal_events: &[ChangelogEvent],
    bookmark: u64,
    record_clean_through: Option<u64>,
    clean_horizon: u64,
) {
    persist_image(
        path,
        &[],
        journal_events,
        Some(bookmark),
        record_clean_through,
        clean_horizon,
    )
    .await;
}

async fn persist_image(
    path: &Path,
    entries: &[WalEntryV2],
    journal_events: &[ChangelogEvent],
    bookmark: Option<u64>,
    record_clean_through: Option<u64>,
    clean_horizon: u64,
) {
    let repo = open_unrecovered(path).await;
    let info = repo.tx_info_store().await.expect("tx info store");
    let top = entries
        .iter()
        .map(|entry| entry.commit_version)
        .chain(journal_events.iter().map(|event| event.commit_version))
        .chain(bookmark)
        .max()
        .unwrap_or(clean_horizon)
        .max(clean_horizon);
    save_last_committed_flushed(&info, top)
        .await
        .expect("persist allocation floor");
    match (bookmark, record_clean_through) {
        (Some(bookmark), Some(known)) => save_bookmark_and_clean_horizon(&info, bookmark, known)
            .await
            .expect("persist follower batch record"),
        (Some(bookmark), None) => save_replication_bookmark(&info, bookmark)
            .await
            .expect("persist follower bookmark"),
        (None, _) => {}
    }
    let wal = repo.repo_wal().await.expect("repo WAL");
    for entry in entries {
        wal.begin_grouped(entry, WalDurability::Buffered)
            .await
            .expect("append WAL entry");
    }
    wal.sync_wal().await.expect("sync WAL entries");
    let store = repo
        .changelog_store_for_cold_reconcile()
        .await
        .expect("changelog store");
    for event in journal_events {
        store
            .set(
                shamir_tx::changefeed::version_key(event.commit_version).into(),
                encode_event(event).expect("encode journal event"),
            )
            .await
            .expect("write journal event");
    }
    store.flush().await.expect("flush fixture journal");
    let adapter = crate::repo::changelog_store::StoreChangelog::new(Arc::clone(&store));
    if clean_horizon > 0 {
        shamir_tx::ChangelogStore::put(
            &adapter,
            clean_horizon_key(),
            shamir_tx::changefeed::encode_clean_horizon(clean_horizon),
        )
        .await
        .expect("persist clean horizon fixture");
    }
    drop(adapter);
    drop(store);
    drop(wal);
    drop(info);
    drop(repo);
}

pub(super) async fn open_and_reconcile(path: &Path) -> DbResult<RepoInstance> {
    let repo = open_unrecovered(path).await;
    let wal = repo.repo_wal().await?;
    let entries = match wal.recover().await {
        Ok(entries) => entries,
        Err(error) => {
            repo.stop_journal_writer_for_test().await?;
            return Err(error);
        }
    };
    if let Err(error) = repo.certify_cold_start(&entries).await {
        repo.stop_journal_writer_for_test().await?;
        return Err(error);
    }
    Ok(repo)
}

pub(super) async fn raw_store(path: &Path, name: &str) -> Arc<dyn Store> {
    let raw = BoxRepoFactory::fjall_raw(path.to_path_buf())
        .create()
        .await
        .expect("open raw factory");
    let store = raw.store_get(name).await.expect("open raw store");
    drop(raw);
    store
}

pub(super) async fn journal_event(store: &dyn Store, version: u64) -> Option<ChangelogEvent> {
    read_event(store, version)
        .await
        .expect("read journal event")
}

pub(super) fn temp_path(temp: &tempfile::TempDir) -> PathBuf {
    temp.path().join("cold_repo")
}
