use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

use crate::tx::tests::post_wal_orphan_ordering_tests::{fixture, panic_post_wal};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repaired_orphan_is_covered_by_explicit_clean_horizon_checkpoint() {
    let (repo, hook) = fixture("orphan_checkpoint_coverage").await;
    let table = repo.get_table("a").await.unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("checkpoint".into()), Some(&mut tx))
        .await
        .unwrap();
    let version = panic_post_wal(&repo, &hook, tx).await;
    drop(snapshot);

    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 1);
    let handle = repo.changefeed().await.unwrap();
    let settled = repo.tx_gate().await.unwrap().completion().watermark();
    handle.feed.record_clean_horizon(settled).await.unwrap();
    assert!(handle.feed.clean_horizon() >= version);

    let (events, gap) = repo
        .read_changelog_for_replication(version, 1)
        .await
        .unwrap();
    assert_eq!(gap, None);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.commit_version, version);
}
