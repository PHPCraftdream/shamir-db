//! Pre-loss follower bookmark versus surviving WAL, with a negative control.
//! The follower mock persists only the maximum page version. The WAL file is
//! truncated only after that observation. H comes from decoding surviving
//! WAL frames, never from the journal or an eager gate floor.

use std::sync::atomic::{AtomicU64, Ordering};

use shamir_wal::{WalDurability, WalSink};

use super::super::helpers::{self, DiskFixture};
use crate::repo::durability_policy::replication_bound::SKIP_S_CLAMP_FOR_TEST;

async fn follower_before_loss(skip_clamp: bool) -> (u64, u64) {
    let DiskFixture {
        repo,
        _dir,
        backing,
    } = helpers::disk_repo("pre_loss_bookmark").await;
    helpers::seed_journal(&repo, &[1, 2, 3]).await;
    helpers::append_wal(&repo, 1, WalDurability::Synced)
        .await
        .unwrap();
    let file_path = backing
        .with_file_name("data.shamirwal")
        .join("00000000.wal");
    let safe_len = std::fs::metadata(&file_path).unwrap().len();
    helpers::append_wal(&repo, 2, WalDurability::Buffered)
        .await
        .unwrap();
    helpers::append_wal(&repo, 3, WalDurability::Buffered)
        .await
        .unwrap();
    let group = repo.wal_group().await.unwrap();
    let _ = group.take_dirty(); // prevent timer admission in this controlled window
    assert_eq!(group.recoverable_through(), 1);

    let bookmark = AtomicU64::new(0); // mock durable follower state
    SKIP_S_CLAMP_FOR_TEST.store(skip_clamp, Ordering::Release);
    if !skip_clamp {
        group.arm_fail_next_sync_for_test();
        WalSink::arm_pause_next_file_sync();
    }
    let pull = {
        let repo = repo.clone();
        tokio::spawn(async move { repo.read_changelog_for_replication(0, 100).await.unwrap() })
    };
    if !skip_clamp {
        WalSink::wait_file_sync_paused().await;
        tokio::time::advance(std::time::Duration::from_millis(200)).await;
    }
    let (events, _) = pull.await.unwrap();
    SKIP_S_CLAMP_FOR_TEST.store(false, Ordering::Release);
    for event in events {
        bookmark.fetch_max(event.event.commit_version, Ordering::AcqRel);
    }
    // The follower has observed/persisted its bookmark BEFORE simulated loss.
    let observed = bookmark.load(Ordering::Acquire);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file_path)
        .unwrap()
        .set_len(safe_len)
        .unwrap();
    if !skip_clamp {
        // Finish shared work after removing the lost tail; join a later demand
        // so no background task remains parked when the fixture is destroyed.
        let remaining = {
            let group = group.clone();
            tokio::spawn(async move { group.sync_now().await })
        };
        group.wait_sync_waiters(1).await;
        WalSink::resume_file_sync();
        assert!(remaining.await.unwrap().is_err());
        assert_eq!(group.recoverable_through(), 1);
    }
    repo.stop_journal_writer_for_test().await.unwrap();
    drop(group);
    drop(repo);
    let reopened = crate::repo::RepoInstance::from_factory(
        "pre_loss_bookmark".into(),
        crate::repo::BoxRepoFactory::fjall_raw(backing),
        vec![crate::table::TableConfig::new("items")],
    )
    .await
    .unwrap();
    let surviving = reopened.repo_wal().await.unwrap().recover().await.unwrap();
    let h = surviving
        .iter()
        .map(|entry| entry.commit_version)
        .max()
        .unwrap_or(0);
    assert_eq!(h, 1, "H must be grounded in surviving WAL frames");
    reopened.stop_journal_writer_for_test().await.unwrap();
    (observed, h)
}

#[tokio::test(start_paused = true)]
async fn follower_bookmark_observed_before_loss_is_within_surviving_horizon() {
    let (bookmark, h) = follower_before_loss(false).await;
    assert_eq!(
        bookmark, 1,
        "non-vacuous: follower observed the durable prefix"
    );
    assert!(bookmark <= h);
}

#[tokio::test(start_paused = true)]
async fn negative_control_pre_loss_bookmark_exceeds_surviving_wal() {
    let (bookmark, h) = follower_before_loss(true).await;
    assert_eq!(bookmark, 3);
    assert!(
        bookmark > h,
        "removing the clamp must violate bookmark safety"
    );
}
