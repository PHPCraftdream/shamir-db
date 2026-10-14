use std::sync::Arc;

use shamir_wal::WalEntryV2;

use super::super::missing_evidence::settle_missing_evidence;
use crate::tx::tests::post_wal_orphan_ordering_tests::fixture;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_gate_and_window_evidence_recovers_then_certifies_gap() {
    let (repo, _hook) = fixture("orphan_missing_evidence").await;
    let gate = repo.tx_gate().await.unwrap();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_accepted();
    drop(guard);
    assert!(gate.is_orphaned_post_wal(version));
    assert!(gate.orphan_entry(version).is_none());

    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 0);
    assert!(!gate.is_orphaned_post_wal(version));
    assert_eq!(repo.drainer().recover_calls(), 1);

    let completion = gate.completion();
    let watermark = completion.watermark();
    let durable_watermark = gate.durable_watermark();
    assert_eq!(watermark, version);
    assert_eq!(durable_watermark, version);
    let handle = repo.changefeed().await.unwrap();
    handle.feed.record_clean_horizon(watermark).await.unwrap();
    assert_eq!(
        handle.feed.certified_gap_interval(),
        Some((version, version))
    );
    handle.feed.shutdown_and_wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_evidence_does_not_settle_version_present_in_window() {
    let (repo, _hook) = fixture("orphan_missing_evidence_window").await;
    let gate = repo.tx_gate().await.unwrap();
    let mut guard = gate.assign_next_version_guarded();
    let version = guard.version();
    guard.wal_accepted();
    drop(guard);

    repo.drainer().offer(Arc::new(
        WalEntryV2::new(version, 0, Vec::new()).with_commit_version(version),
    ));
    let drainer = repo.drainer();
    settle_missing_evidence(&drainer, &repo, &gate)
        .await
        .unwrap();
    assert!(gate.is_orphaned_post_wal(version));
    assert_eq!(gate.durable_watermark(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_evidence_does_not_settle_non_contiguous_orphan() {
    let (repo, _hook) = fixture("orphan_missing_evidence_non_contiguous").await;
    let gate = repo.tx_gate().await.unwrap();
    let first = gate.assign_next_version_guarded();
    let first_version = first.version();
    let mut second = gate.assign_next_version_guarded();
    let second_version = second.version();
    second.wal_accepted();
    drop(second);

    let drainer = repo.drainer();
    settle_missing_evidence(&drainer, &repo, &gate)
        .await
        .unwrap();
    assert!(gate.is_orphaned_post_wal(second_version));
    assert_eq!(gate.durable_watermark(), 0);

    drop(first);
    assert_eq!(repo.drainer().drain_all(&repo).await.unwrap(), 0);
    assert!(!gate.is_orphaned_post_wal(second_version));
    assert_eq!(gate.durable_watermark(), second_version);
    assert_eq!(
        repo.changefeed().await.unwrap().feed.first_gap_version(),
        second_version
    );
    assert!(first_version < second_version);
}
