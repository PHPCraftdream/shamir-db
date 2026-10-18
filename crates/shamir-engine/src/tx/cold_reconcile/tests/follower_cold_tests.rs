//! PR6c regression tests: at follower cold start a bookmark record with a
//! known journal horizon J certifies everything through J, so the uncertain
//! GAP is exactly `in_flight_range(J, recovered floor)` — or absent when the
//! in-flight batch's J+bookmark record landed.

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, FollowerRefusalReason};
use shamir_storage::types::RecordKey;
use shamir_tx::changefeed::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;

use super::super::follower::ignore_bookmark_j;
use super::helpers::{
    open_and_reconcile, prepare_follower_image, prepare_image, raw_store, temp_path,
};
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::follower_durability::test_seam::{self, Phase};
use crate::tx::apply_replicated::{apply_replicated_batch, ApplyOutcome, ApplyPolicy};
use crate::tx::ReplicatedEvent;

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

fn replicated(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: {
                    let mut raw = [0u8; 16];
                    raw[0] = id;
                    Bytes::copy_from_slice(&raw)
                },
                op: ChangeOp::Put,
                value: Some(Bytes::from_static(b"payload")),
            }],
        },
        field_bindings: Vec::new(),
    }
}

async fn open(path: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.to_owned(),
        BoxRepoFactory::fjall_raw(path.to_path_buf()),
        vec![TableConfig::new("items")],
    )
    .await
    .expect("open fjall repo")
}

async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.expect("changefeed");
    handle.feed.shutdown_and_wait().await.expect("writer stop");
    drop(handle);
    drop(repo);
}

async fn stop(repo: RepoInstance) {
    repo.stop_journal_writer_for_test()
        .await
        .expect("stop journal writer");
    drop(repo);
}

// Failure points of a follower batch, each reopened twice through the REAL
// open path (`from_factory` runs recovery + cold certification). The receipt
// pipeline makes the whole batch atomic: a batch failing at ANY receipt
// phase leaves an outstanding durable receipt, and the cold reopen
// COMPLETES it — the in-flight batch's version is certified through the
// bookmark's clean horizon and is never declared a recovery GAP. Graceful
// shutdown validates persisted state; it is not a power-cut simulation.
#[tokio::test(flavor = "current_thread")]
async fn follower_batch_crash_points_certify_only_the_in_flight_batch() {
    // The two PRE-checkpoint phases leave a durable receipt outstanding;
    // at `Phase::Checkpointed` the bookmark already advanced (nothing is in
    // flight), which the crash-subprocess matrix covers.
    for (tag, phase) in [
        ("prepared", Phase::Prepared),
        ("materialized", Phase::Materialized),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let name = format!(
            "follower-cold-{tag}-{}",
            temp.path().display().to_string().replace('\\', "_")
        );
        let repo = open(temp.path(), &name).await;
        assert_eq!(
            apply_replicated_batch(&repo, &[replicated(1, 1)], 0, ApplyPolicy::Trusted)
                .await
                .expect("first batch applies"),
            ApplyOutcome::Applied { local_version: 1 }
        );
        // A sticky `fail_after` at the phase boundary exhausts the receipt
        // pipeline's 3 Storage retries: the worker parks the chunk guards,
        // leaves the durable receipt outstanding (the same durable state a
        // crash leaves), returns Err, and goes IDLE.
        let hook = Arc::new(test_seam::Hook::default());
        *hook.fail_after.lock().unwrap() = Some(phase);
        hook.fail_after_sticky
            .store(true, std::sync::atomic::Ordering::Release);
        test_seam::install(&name, Arc::clone(&hook));
        let error = apply_replicated_batch(&repo, &[replicated(2, 2)], 1, ApplyPolicy::Trusted)
            .await
            .expect_err("the injected failure must fail the second batch");
        assert!(
            matches!(error, shamir_storage::error::DbError::Storage(_)),
            "expected the injected Storage failure at {tag}: {error:?}"
        );
        test_seam::remove(&name);
        // The parked receipt is a DURABLE PAUSE: shutdown must report it and
        // fail the gate for the next open.
        let shutdown = repo.shutdown_clean(std::time::Duration::from_secs(5)).await;
        assert!(
            matches!(&shutdown, Err(shamir_storage::error::DbError::Storage(message)) if message.contains("durable pause")),
            "shutdown must report the durable pause at {tag}, got {shutdown:?}"
        );
        assert!(repo.tx_gate().await.expect("gate").requires_reopen());
        close(repo).await;
        for _ in 0..2 {
            let reopened = open(temp.path(), &name).await;
            let feed = reopened.changefeed().await.expect("changefeed").feed;
            // The cold reconcile COMPLETES the outstanding receipt: the
            // in-flight batch's version is certified, so the gap is NONE at
            // every receipt crash point — never an in-flight gap.
            assert_eq!(
                feed.certified_gap_interval(),
                None,
                "cold-start gap at {tag}"
            );
            let page = reopened.read_changelog_from(1, 8).await.expect("pull page");
            let served_versions: Vec<u64> = page
                .events
                .iter()
                .map(|event| event.commit_version)
                .collect();
            assert_eq!(served_versions, vec![1, 2], "pull events at {tag}");
            assert_eq!(page.gap_at, None, "pull gap at {tag}");
            drop(feed);
            close(reopened).await;
        }
    }
}

// A v3 record without a known J keeps the conservative whole-window GAP.
// Regression guard: must stay true.
#[tokio::test(flavor = "current_thread")]
async fn unknown_j_record_keeps_the_conservative_gap() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_follower_image(&path, &[event(3)], 10, None, 0).await;
    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let feed = repo.changefeed().await.unwrap().feed;
        assert_eq!(feed.certified_gap_interval(), Some((1, 10)));
        let page = repo.read_changelog_from(1, 8).await.unwrap();
        assert_eq!(page.gap_at, Some(1));
        // The reader serves events that exist up to the confirmed bound and
        // flags the gap separately: version 3's event is present inside the
        // wide legacy GAP (1, 10] and IS returned, and no event outside the
        // recorded journal is silently invented.
        assert_eq!(page.events, vec![event(3)]);
        drop(feed);
        stop(repo).await;
    }
}

// A record with a known J certifies events up to J: the GAP after reopen is
// exactly the tail after J. FAILS on pre-PR6c code, which ignores the
// record and marks the whole (1, 10] window.
#[tokio::test(flavor = "current_thread")]
async fn known_j_record_gaps_only_the_in_flight_tail_reopen_twice() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_follower_image(&path, &[event(1), event(2), event(3)], 10, Some(3), 0).await;
    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let feed = repo.changefeed().await.unwrap().feed;
        assert_eq!(feed.certified_gap_interval(), Some((4, 10)));
        let page = repo.read_changelog_from(1, 8).await.unwrap();
        assert_eq!(page.gap_at, Some(4));
        assert_eq!(page.events, vec![event(1), event(2), event(3)]);
        drop(feed);
        stop(repo).await;
    }
}

// NEGATIVE CONTROL: with the protection seam switched off the same image
// reproduces the wide pre-PR6c GAP, proving the assertion above can tell
// the two behaviours apart.
#[tokio::test(flavor = "current_thread")]
async fn ignoring_the_record_j_reproduces_the_wide_gap() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_follower_image(&path, &[event(3)], 10, Some(3), 0).await;
    ignore_bookmark_j(true);
    let repo = open_and_reconcile(&path).await.unwrap();
    ignore_bookmark_j(false);
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(feed.certified_gap_interval(), Some((1, 10)));
    drop(feed);
    stop(repo).await;
}

// A repository without follower provenance is untouched: a journal event
// with no WAL evidence is still deleted and conservatively gapped.
#[tokio::test(flavor = "current_thread")]
async fn non_follower_repos_keep_exact_conservative_certification() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp_path(&temp);
    prepare_image(&path, &[], &[event(1)], None, 0).await;
    for _ in 0..2 {
        let repo = open_and_reconcile(&path).await.unwrap();
        let feed = repo.changefeed().await.unwrap().feed;
        assert_eq!(feed.certified_gap_interval(), Some((1, 1)));
        let page = repo.read_changelog_from(1, 8).await.unwrap();
        assert_eq!(page.gap_at, Some(1));
        assert!(page.events.is_empty());
        drop(feed);
        stop(repo).await;
    }
}

// Cold ordering (S5c-X PR6b-3): a legacy bookmark record in `__tx__` must
// REFUSE the reopen (typed `FollowerStateRefused`) BEFORE WAL replay and
// publication run: after the failed open the durable `LastCommittedVersion`
// floor and the changelog clean horizon are unchanged, the legacy bytes are
// untouched, and no recovery gap marker was written.
#[cfg(feature = "fjall")]
#[tokio::test(flavor = "current_thread")]
async fn legacy_bookmark_refuses_reopen_before_replay() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = format!(
        "cold-legacy-refusal-{}",
        temp.path().display().to_string().replace('\\', "_")
    );
    let repo = open(temp.path(), &name).await;
    // REAL in-flight WAL evidence: a committed tx (receipt pipeline) whose
    // WAL entry survives the close.
    assert_eq!(
        apply_replicated_batch(&repo, &[replicated(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .expect("the seed batch applies"),
        ApplyOutcome::Applied { local_version: 1 }
    );
    let info = repo.tx_info_store().await.expect("tx info store");
    let legacy = crate::meta::MetaEnvelope::new(10u64).encode().unwrap();
    let bookmark_key = RecordKey::from_slice(
        crate::meta::MetaKey::ReplicationBookmark
            .as_record_id()
            .as_bytes(),
    );
    info.set(bookmark_key.clone(), Bytes::from(legacy.clone()))
        .await
        .expect("inject legacy bookmark bytes");
    drop(info);
    close(repo).await;

    // Baseline from the raw stores AFTER the close, BEFORE the refused open.
    let tx = raw_store(temp.path(), "__tx__").await;
    let floor_before = crate::meta::recovery_marker::load_last_committed(&tx)
        .await
        .expect("floor baseline");
    let legacy_before = tx.get(bookmark_key.clone()).await.expect("legacy bytes");
    let changelog = raw_store(temp.path(), "__changelog__").await;
    let clean_before = changelog
        .get(RecordKey::from_slice(
            &shamir_tx::changefeed::clean_horizon_key(),
        ))
        .await
        .ok();
    let gap_before = changelog
        .get(RecordKey::from_slice(
            &shamir_tx::changefeed::gap_marker_key(),
        ))
        .await
        .ok();

    // The open must FAIL FAST — no retry, no replay.
    let error = RepoInstance::from_factory(
        name.clone(),
        BoxRepoFactory::fjall_raw(temp.path().to_path_buf()),
        vec![TableConfig::new("items")],
    )
    .await;
    let error = match error {
        Ok(_) => panic!("legacy follower store must refuse the open"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            DbError::FollowerStateRefused {
                reason: FollowerRefusalReason::LegacyBookmarkFormat
            }
        ),
        "{error:?}"
    );

    // No replay/publication side effects after the failed open.
    let tx_after = raw_store(temp.path(), "__tx__").await;
    assert_eq!(
        crate::meta::recovery_marker::load_last_committed(&tx_after)
            .await
            .expect("floor after"),
        floor_before,
        "the durable floor must be unchanged"
    );
    assert_eq!(
        tx_after
            .get(bookmark_key.clone())
            .await
            .expect("bytes after"),
        legacy_before,
        "the legacy bookmark bytes must be untouched"
    );
    let changelog_after = raw_store(temp.path(), "__changelog__").await;
    assert_eq!(
        changelog_after
            .get(RecordKey::from_slice(
                &shamir_tx::changefeed::clean_horizon_key()
            ))
            .await
            .ok(),
        clean_before,
        "the clean horizon must be unchanged"
    );
    assert_eq!(
        changelog_after
            .get(RecordKey::from_slice(
                &shamir_tx::changefeed::gap_marker_key()
            ))
            .await
            .ok(),
        gap_before,
        "no gap marker may be written"
    );
}
