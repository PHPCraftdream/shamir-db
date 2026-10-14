use super::helpers::{
    open_and_reconcile, open_and_reconcile_with_hook, open_unrecovered, prepare_image, temp_path,
};
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::clean_horizon::checkpoint::test_seam;
use crate::tx::drainer::Drainer;
use bytes::Bytes;
use shamir_query_builder::write::doc;
use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};
use shamir_storage::error::DbError;
use shamir_tx::changefeed::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_wal::{WalEntryV2, WalEventHeader};

fn event(version: u64) -> ChangelogEvent {
    ChangelogEvent {
        repo: super::helpers::REPO_NAME.into(),
        commit_version: version,
        tx_id: 17,
        actor: Actor::System,
        timestamp_ns: 23,
        changes: vec![RecordChange {
            table: "items".into(),
            key: Bytes::from_static(b"record-key-00001"),
            op: ChangeOp::Put,
            value: Some(Bytes::from_static(b"payload")),
        }],
    }
}

fn derived(version: u64) -> WalEntryV2 {
    WalEntryV2::new(version, 0, Vec::new())
        .with_commit_version(version)
        .with_event_header(WalEventHeader {
            repo: super::helpers::REPO_NAME.into(),
            actor: Actor::System,
            tx_id: 17,
            timestamp_ns: 23,
            table_names: vec![(1, "items".into())],
            extra_changes: vec![shamir_wal::WalExtraChange {
                table_id_interned: 1,
                change_index: 0,
                key: Bytes::from_static(b"record-key-00001"),
                value: Some(Bytes::from_static(b"payload")),
            }],
        })
}

fn eventless(version: u64) -> WalEntryV2 {
    WalEntryV2::new(version, 0, Vec::new())
        .with_commit_version(version)
        .with_event_header(WalEventHeader {
            repo: super::helpers::REPO_NAME.into(),
            actor: Actor::System,
            tx_id: 0,
            timestamp_ns: 23,
            table_names: Vec::new(),
            extra_changes: Vec::new(),
        })
}

async fn stop(repo: RepoInstance) {
    repo.stop_journal_writer_for_test()
        .await
        .expect("stop journal writer");
    drop(repo);
}

fn disk_policy(action: RetentionOverflow, segment_bytes: u64) -> crate::repo::DurabilityPolicy {
    crate::repo::DurabilityPolicy::new(
        WalCommitTier::Buffered,
        segment_bytes.saturating_mul(1024).max(1),
        action,
        ReplicationServe::Durable,
    )
}

async fn seeded_retention_repo(path: &std::path::Path, action: RetentionOverflow) -> RepoInstance {
    let segment_bytes = 256;
    let repo = RepoInstance::from_factory_with_test_segment_bytes(
        super::helpers::REPO_NAME.into(),
        crate::repo::BoxRepoFactory::fjall_raw(path.to_path_buf()),
        vec![TableConfig::new("items")],
        disk_policy(action, segment_bytes),
        segment_bytes,
    )
    .await
    .expect("open retention fixture repo");
    repo.disable_background_drainer_for_test();
    for i in 0..8 {
        let (mut tx, _guard) = repo
            .begin_tx(crate::tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        let user_record = doc()
            .set("payload", format!("row-{i}-{}", "x".repeat(512)))
            .build();
        let table = repo.get_table("items").await.unwrap();
        let interner = table.interner().get().await.unwrap();
        let record = query_value_to_inner(&user_record, interner).unwrap();
        table.insert_tx(&record, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
    }
    repo
}

#[tokio::test(flavor = "current_thread")]
async fn surviving_v3_wal_repairs_missing_event_and_follower_cursor_reads_it_twice() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_image(&path, &[derived(1)], &[], None, 0).await;

    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let read = repo.read_changelog_from(1, 8).await.unwrap();
        assert!(read.gap_at.is_none());
        assert_eq!(read.events, vec![event(1)]);
        stop(repo).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_wal_is_point_gap_and_eventless_wal_is_gap_free() {
    let temp = tempfile::tempdir().unwrap();
    let legacy_path = temp.path().join("legacy");
    let legacy = WalEntryV2::new(1, 0, Vec::new()).with_commit_version(1);
    prepare_image(&legacy_path, &[legacy], &[], None, 0).await;
    let repo = open_and_reconcile(&legacy_path).await.unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(feed.certified_gap_interval(), Some((1, 1)));
    stop(repo).await;

    let eventless_path = temp.path().join("eventless");
    prepare_image(&eventless_path, &[eventless(1)], &[], None, 0).await;
    let repo = open_and_reconcile(&eventless_path).await.unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(feed.first_gap_version(), 0);
    assert_eq!(feed.certified_gap_interval(), None);
    stop(repo).await;
}

#[tokio::test(flavor = "current_thread")]
async fn conflicting_journal_event_refuses_open() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    let conflict = ChangelogEvent {
        tx_id: 999,
        ..event(1)
    };
    prepare_image(&path, &[derived(1)], &[conflict], None, 0).await;
    assert!(open_and_reconcile(&path).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn lost_unserved_event_is_deleted_but_provenance_keeps_event_and_gaps() {
    let temp = tempfile::tempdir().unwrap();
    let lost_path = temp.path().join("lost");
    prepare_image(&lost_path, &[], &[event(1)], None, 0).await;
    let repo = open_and_reconcile(&lost_path).await.unwrap();
    let store = repo.changelog_store_for_cold_reconcile().await.unwrap();
    let lost_read = repo.read_changelog_from(1, 8).await.unwrap();
    assert_eq!(lost_read.gap_at, Some(1));
    assert!(super::helpers::journal_event(store.as_ref(), 1)
        .await
        .is_none());
    drop(store);
    stop(repo).await;

    let served_path = temp.path().join("served");
    prepare_image(&served_path, &[], &[event(1)], Some(1), 0).await;
    let repo = open_and_reconcile(&served_path).await.unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    let store = repo.changelog_store_for_cold_reconcile().await.unwrap();
    assert!(super::helpers::journal_event(store.as_ref(), 1)
        .await
        .is_some());
    assert_eq!(feed.certified_gap_interval(), Some((1, 1)));
    drop(store);
    stop(repo).await;
}

#[tokio::test(flavor = "current_thread")]
async fn corrupt_journal_event_refuses_open() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_image(&path, &[derived(1)], &[], None, 0).await;
    let store = super::helpers::raw_store(&path, "__changelog__").await;
    store
        .set(
            shamir_tx::changefeed::version_key(1).into(),
            Bytes::from_static(b"not-msgpack"),
        )
        .await
        .unwrap();
    store.flush().await.unwrap();
    drop(store);
    assert!(open_and_reconcile(&path).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn reconciliation_interruption_points_converge_on_later_open() {
    for (case, entries, journal, phase) in [
        ("pre_put", vec![derived(1)], vec![], "pre_put"),
        ("after_put", vec![derived(1)], vec![], "after_put"),
        ("pre_delete", vec![], vec![event(1)], "pre_delete"),
        ("after_delete", vec![], vec![event(1)], "after_delete"),
        ("pre_flush", vec![derived(1)], vec![], "pre_flush"),
        ("after_flush", vec![derived(1)], vec![], "after_flush"),
        ("before_j", vec![derived(1)], vec![], "before_j"),
        ("after_j", vec![derived(1)], vec![], "after_j"),
        ("mid_reconcile", vec![derived(1)], vec![], "mid_reconcile"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(case);
        prepare_image(&path, &entries, &journal, None, 0).await;
        let injected = phase.to_string();
        let result = open_and_reconcile_with_hook(&path, move |at| {
            if at == injected {
                return Err(DbError::Internal(format!("crash at {at}")));
            }
            Ok(())
        })
        .await;
        assert!(result.is_err(), "fault must fire at {phase}");

        for _ in 0..2 {
            let repo = open_and_reconcile(&path).await.unwrap();
            let read = repo.read_changelog_from(1, 8).await.unwrap();
            let store = repo.changelog_store_for_cold_reconcile().await.unwrap();
            assert_eq!(super::helpers::clean_horizon(store.as_ref()).await, 1);
            if case == "pre_delete" || case == "after_delete" {
                assert_eq!(read.gap_at, Some(1), "lost event gap after {case}");
                assert!(read.events.is_empty(), "deleted event after {case}");
                assert!(super::helpers::journal_event(store.as_ref(), 1)
                    .await
                    .is_none());
            } else {
                assert_eq!(read.gap_at, None, "no follower gap after {case}");
                assert_eq!(read.events, vec![event(1)], "event after {case}");
                assert!(super::helpers::journal_event(store.as_ref(), 1)
                    .await
                    .is_some());
            }
            drop(store);
            stop(repo).await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn stopped_image_crash_boundaries_reopen_twice_without_follower_gaps() {
    // These are deterministic persisted-image states, not claims of process
    // crashes at the named instruction. PR2's crash harness separately
    // exercises actual pre/post-truncate process termination.
    let cases = [
        ("pre_publish", vec![derived(1)], vec![], 0),
        ("pre_truncate", vec![derived(1)], vec![event(1)], 0),
        ("post_truncate", vec![], vec![event(1)], 1),
        ("mid_checkpoint", vec![derived(1)], vec![event(1)], 0),
    ];
    for (boundary, entries, journal, clean_horizon) in cases {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(boundary);
        prepare_image(&path, &entries, &journal, None, clean_horizon).await;
        for _ in 0..2 {
            let repo = open_and_reconcile(&path).await.unwrap();
            let page = repo.read_changelog_from(1, 8).await.unwrap();
            assert_eq!(page.gap_at, None, "follower gap at {boundary}");
            assert_eq!(page.events, vec![event(1)], "follower event at {boundary}");
            stop(repo).await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_boundary_images_converge_twice_without_follower_gap() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mid_checkpoint");
    prepare_image(&path, &[derived(1)], &[event(1)], None, 0).await;

    let first = open_unrecovered(&path).await;
    let wal = first.repo_wal().await.unwrap();
    let entries = wal.recover().await.unwrap();
    assert!(!entries.is_empty());
    drop(wal);
    let page = first.read_changelog_from(1, 8).await.unwrap();
    assert_eq!(page.gap_at, None);
    assert_eq!(page.events, vec![event(1)]);
    let feed = first.changefeed().await.unwrap().feed;
    assert_eq!(feed.clean_horizon(), 0);
    stop(first).await;

    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let page = repo.read_changelog_from(1, 8).await.unwrap();
        assert_eq!(page.gap_at, None);
        assert_eq!(page.events, vec![event(1)]);
        stop(repo).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mid_reconcile_image_converges_twice_after_interrupted_put() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_image(&path, &[derived(1)], &[], None, 0).await;
    let result = open_and_reconcile_with_hook(&path, |phase| {
        if phase == "mid_reconcile" {
            return Err(DbError::Internal("simulated mid-reconcile stop".into()));
        }
        Ok(())
    })
    .await;
    assert!(result.is_err());
    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let page = repo.read_changelog_from(1, 8).await.unwrap();
        assert_eq!(page.gap_at, None);
        assert_eq!(page.events, vec![event(1)]);
        stop(repo).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn retention_checkpoint_protects_wal_and_bypass_control_exposes_loss() {
    let temp = tempfile::tempdir().unwrap();

    let protected_path = temp.path().join("protected");
    let protected = seeded_retention_repo(&protected_path, RetentionOverflow::RefuseWrites).await;
    let protected_feed = protected.changefeed().await.unwrap().feed;
    let protected_wal = protected.repo_wal().await.unwrap();
    assert!(protected_wal.has_truncatable(u64::MAX));
    let protected_count = protected_wal.recover().await.unwrap().len();
    assert!(protected_count > 0);
    let protected_gate = protected.tx_gate().await.unwrap();
    protected_gate.mark_durable(protected_gate.last_committed());
    drop(protected_gate);
    for _ in 0..8 {
        let store = protected
            .changelog_store_for_cold_reconcile()
            .await
            .unwrap();
        assert!(store.flush().await.is_ok());
        drop(store);
        Drainer::new().drain_all(&protected).await.unwrap();
    }
    let protected_reads = protected.read_changelog_from(1, usize::MAX).await.unwrap();
    assert!(protected_reads.gap_at.is_none());
    let protected_events = protected_reads.events;
    assert!(!protected_events.is_empty());
    assert!(protected_wal.recover().await.unwrap().len() < protected_count);
    drop(protected_feed);
    drop(protected_wal);
    stop(protected).await;
    for _ in 0..2 {
        let reopened = open_and_reconcile(&protected_path).await.unwrap();
        let read = reopened.read_changelog_from(1, usize::MAX).await.unwrap();
        assert_eq!(read.gap_at, None);
        assert_eq!(read.events, protected_events);
        stop(reopened).await;
    }

    let bypass_path = temp.path().join("bypass");
    let bypass = seeded_retention_repo(&bypass_path, RetentionOverflow::RefuseWrites).await;
    let bypass_feed = bypass.changefeed().await.unwrap().feed;
    let bypass_wal = bypass.repo_wal().await.unwrap();
    assert!(bypass_wal.has_truncatable(u64::MAX));
    let before = bypass_wal.recover().await.unwrap().len();
    let gate = bypass.tx_gate().await.unwrap();
    gate.mark_durable(gate.last_committed());
    drop(gate);
    test_seam::with_state(&bypass, |state| state.bypass_checkpoint = true);
    for _ in 0..8 {
        Drainer::new().drain_all(&bypass).await.unwrap();
    }
    assert_eq!(bypass_feed.clean_horizon(), 0);
    let bypass_reads = bypass.read_changelog_from(1, usize::MAX).await.unwrap();
    assert!(bypass_reads.gap_at.is_none());
    let bypass_events = bypass_reads.events;
    assert_eq!(bypass_events.len(), 8);
    let surviving_wal = bypass_wal.recover().await.unwrap();
    let absent_wal_events: Vec<_> = bypass_events
        .iter()
        .filter(|event| {
            !surviving_wal
                .iter()
                .any(|entry| entry.commit_version == event.commit_version)
        })
        .cloned()
        .collect();
    assert!(!absent_wal_events.is_empty());
    let lost_event = absent_wal_events
        .first()
        .expect("at least one journal event lost its WAL evidence")
        .clone();
    let expected_after_loss: Vec<_> = bypass_events
        .iter()
        .filter(|event| {
            surviving_wal
                .iter()
                .any(|entry| entry.commit_version == event.commit_version)
        })
        .cloned()
        .collect();
    assert!(surviving_wal.len() < before);
    drop(bypass_feed);
    drop(bypass_wal);
    stop(bypass).await;

    // Model loss of an unflushed journal tail after its WAL segment was
    // truncated. This deliberately edits the stopped disk image, not the
    // normal journal writer path or the checkpoint-protected variant.
    let store = super::helpers::raw_store(&bypass_path, "__changelog__").await;
    shamir_tx::changefeed::cold::delete_event(store.as_ref(), lost_event.commit_version)
        .await
        .unwrap();
    store.flush().await.unwrap();
    drop(store);

    for _ in 0..2 {
        let reopened = open_and_reconcile(&bypass_path).await.unwrap();
        let feed = reopened.changefeed().await.unwrap().feed;
        assert!(feed.first_gap_version() > 0);
        let read = reopened.read_changelog_from(1, usize::MAX).await.unwrap();
        assert_eq!(read.gap_at, Some(lost_event.commit_version));
        assert_eq!(read.events, expected_after_loss);
        stop(reopened).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ephemeral_repositories_keep_legacy_cold_start_behavior() {
    let raw = crate::repo::BoxRepoFactory::in_memory()
        .create()
        .await
        .unwrap();
    let repo = RepoInstance::new(
        "ephemeral_cold_reconcile".into(),
        raw,
        vec![TableConfig::new("items")],
    );
    let wal = repo.repo_wal().await.unwrap();
    assert!(wal.is_mem_sink());
    let entry = derived(1);
    wal.begin_grouped(&entry, shamir_wal::WalDurability::Buffered)
        .await
        .unwrap();
    repo.certify_cold_start(&[entry]).await.unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(feed.clean_horizon(), 0);
    assert_eq!(feed.first_gap_version(), 0);
    stop(repo).await;
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_factory_keeps_legacy_cold_start_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let raw = crate::repo::BoxRepoFactory::hybrid(temp.path())
        .create()
        .await
        .unwrap();
    let repo = RepoInstance::new(
        "hybrid_cold_reconcile".into(),
        raw,
        vec![TableConfig::new("items")],
    );
    let wal = repo.repo_wal().await.unwrap();
    assert!(wal.is_mem_sink());
    let entry = derived(1);
    wal.begin_grouped(&entry, shamir_wal::WalDurability::Buffered)
        .await
        .unwrap();
    repo.certify_cold_start(&[entry]).await.unwrap();
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(feed.clean_horizon(), 0);
    assert_eq!(feed.first_gap_version(), 0);
    stop(repo).await;
}
