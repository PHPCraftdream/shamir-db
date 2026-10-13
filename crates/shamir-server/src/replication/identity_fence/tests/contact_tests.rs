//! First Hello pins identity before Pull; ephemeral backends recreate __tx__.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use shamir_db::engine::meta::repo_identity::RepoIdentity;
use shamir_db::engine::meta::{MetaEnvelope, MetaKey};
use shamir_db::engine::repo::BoxRepoFactory;
use shamir_db::storage::types::RecordKey;
use shamir_db::SeedRefusal;
use shamir_query_types::wire::repl::ReplResponse;

use super::identity_fence_tests::{build_factory, build_memory, repo, write_rows};
use crate::replication::follower_loop::{run_follower_loop, FollowerLoopConfig};
use crate::replication::identity_fence::{checks, recorded};
use crate::replication::in_process::InProcessReplSource;
use crate::replication::source::ReplSource;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn hello_records_actual_bookmark_value_before_any_pull() {
    let leader = build_memory().await;
    let follower = build_memory().await;
    let hello = InProcessReplSource::new(leader.clone())
        .hello("probe")
        .await
        .unwrap();
    checks::fence_hello_recorded(&follower, "app", "main", &hello, None)
        .await
        .unwrap();
    let info = repo(&follower).tx_info_store().await.unwrap();
    let bytes = info
        .get(RecordKey::from_slice(
            MetaKey::ReplicationBookmark.as_record_id().as_bytes(),
        ))
        .await
        .unwrap();
    let (version, record): (u16, recorded::FenceRecord) = MetaEnvelope::open(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(record.bookmark, 0);
    assert_eq!(
        record.lineage_id,
        repo(&leader).repo_identity().await.unwrap().lineage_id
    );
    repo(&follower)
        .advance_replication_bookmark(17)
        .await
        .unwrap();
    let advanced = recorded::load(&info).await.unwrap().unwrap();
    assert_eq!(advanced.bookmark, 17);
    assert_eq!(advanced.lineage_id, record.lineage_id);
}

#[tokio::test]
async fn changed_first_pull_cannot_replace_first_hello_identity() {
    let first = build_memory().await;
    let regenerated = build_memory().await;
    write_rows(&regenerated, 10, 4).await;
    let follower = build_memory().await;
    let source = Arc::new(super::identity_fence_tests::StaticHelloSource {
        hello: InProcessReplSource::new(first)
            .hello("probe")
            .await
            .unwrap(),
        inner: Arc::new(InProcessReplSource::new(regenerated)),
        pulls: Default::default(),
    });
    let before = repo(&follower).current_commit_version().await.unwrap();
    let error = run_follower_loop(
        Arc::new(follower.clone()),
        source.clone(),
        FollowerLoopConfig::new("probe", "app", "main")
            .with_poll_wait_ms(0)
            .with_max_iterations(1),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    super::identity_fence_tests::assert_identity_mismatch(&error, SeedRefusal::WrongIdentity);
    assert_eq!(source.pulls.load(Ordering::Acquire), 1);
    assert_eq!(
        repo(&follower).current_commit_version().await.unwrap(),
        before
    );
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
}

#[tokio::test]
async fn missing_identity_is_fail_closed_even_without_a_record() {
    let leader = build_memory().await;
    let follower = build_memory().await;
    let source = InProcessReplSource::new(leader);
    let mut hello = source.hello("probe").await.unwrap();
    if let ReplResponse::Hello { repos, .. } = &mut hello {
        repos[0].repo_lineage_id = None;
    }
    assert!(matches!(
        checks::fence_hello_recorded(&follower, "app", "main", &hello, None).await,
        Err(crate::replication::ReplError::Seed(
            SeedRefusal::MissingIdentity
        ))
    ));
    let mut pull = source.pull("app", "main", 1, 1000, Some(0)).await.unwrap();
    if let ReplResponse::Pull { history_epoch, .. } = &mut pull {
        *history_epoch = None;
    }
    let info = repo(&follower).tx_info_store().await.unwrap();
    assert!(matches!(
        checks::fence_pull_identity(&info, &pull, None).await,
        Err(crate::replication::ReplError::Seed(
            SeedRefusal::MissingIdentity
        ))
    ));
    assert_eq!(recorded::load(&info).await.unwrap(), None);
}

#[tokio::test]
async fn hello_history_epoch_change_stops_before_pull() {
    let leader = build_memory().await;
    let follower = build_memory().await;
    let source = InProcessReplSource::new(leader.clone());
    checks::fence_hello_recorded(
        &follower,
        "app",
        "main",
        &source.hello("probe").await.unwrap(),
        None,
    )
    .await
    .unwrap();
    repo(&leader).bump_history_epoch().await.unwrap();
    let error = checks::fence_hello_recorded(
        &follower,
        "app",
        "main",
        &source.hello("probe").await.unwrap(),
        None,
    )
    .await
    .unwrap_err();
    super::identity_fence_tests::assert_identity_mismatch(&error, SeedRefusal::WrongHistoryEpoch);
    assert_eq!(repo(&follower).replication_bookmark().await.unwrap(), 0);
}

#[test]
fn memory_and_hybrid_reopen_with_new_lineage_and_stop_followers() {
    for hybrid in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hybrid");
        let factory = || {
            if hybrid {
                BoxRepoFactory::hybrid(&path)
            } else {
                BoxRepoFactory::in_memory()
            }
        };
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        };
        let rt = runtime();
        let identity = rt.block_on(async {
            let leader = build_factory(factory()).await;
            repo(&leader).repo_identity().await.unwrap()
        });
        drop(rt);
        let rt = runtime();
        rt.block_on(async {
            let leader = build_factory(factory()).await;
            let observed = repo(&leader).repo_identity().await.unwrap();
            assert_ne!(identity.lineage_id, observed.lineage_id);
            assert_eq!(observed.history_epoch, 0);
            let follower = build_memory().await;
            let info = repo(&follower).tx_info_store().await.unwrap();
            recorded::save(
                &info,
                recorded::FenceRecord {
                    bookmark: 0,
                    lineage_id: identity.lineage_id,
                    history_epoch: identity.history_epoch,
                    observed_identity: Some(identity),
                },
            )
            .await
            .unwrap();
            let before = repo(&follower).current_commit_version().await.unwrap();
            let error = run_follower_loop(
                Arc::new(follower.clone()),
                Arc::new(InProcessReplSource::new(leader)),
                FollowerLoopConfig::new("probe", "app", "main").with_max_iterations(1),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
            super::identity_fence_tests::assert_identity_mismatch(
                &error,
                SeedRefusal::WrongIdentity,
            );
            assert_eq!(
                repo(&follower).current_commit_version().await.unwrap(),
                before
            );
        });
        drop(rt);
    }
}

#[tokio::test]
async fn bookmark_codec_round_trips_boundaries_and_rejects_trailing_bytes() {
    let follower = build_memory().await;
    let info = repo(&follower).tx_info_store().await.unwrap();
    for bookmark in [0, u64::MAX] {
        for observed_identity in [
            None,
            Some(RepoIdentity {
                lineage_id: [255; 16],
                history_epoch: u64::MAX,
            }),
        ] {
            let record = recorded::FenceRecord {
                bookmark,
                lineage_id: [17; 16],
                history_epoch: u64::MAX,
                observed_identity,
            };
            recorded::save(&info, record).await.unwrap();
            assert_eq!(recorded::load(&info).await.unwrap(), Some(record));
            assert_eq!(
                repo(&follower).replication_bookmark().await.unwrap(),
                bookmark
            );
            let mut bytes = MetaEnvelope::new((1u16, record)).encode().unwrap();
            bytes.push(0);
            info.set(
                RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes()),
                Bytes::from(bytes),
            )
            .await
            .unwrap();
            assert!(recorded::load(&info).await.is_err());
        }
    }
}
