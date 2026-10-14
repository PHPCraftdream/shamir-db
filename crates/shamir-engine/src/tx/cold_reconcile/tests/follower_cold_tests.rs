//! PR6c regression tests: at follower cold start a bookmark record with a
//! known journal horizon J certifies everything through J, so the uncertain
//! GAP is exactly `in_flight_range(J, recovered floor)` — or absent when the
//! in-flight batch's J+bookmark record landed.

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::changefeed::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;

use super::super::follower::ignore_bookmark_j;
use super::helpers::{open_and_reconcile, prepare_follower_image, prepare_image, temp_path};
use crate::meta::repl_bookmark::load_clean_horizon;
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

// Crash points of a follower batch, each reopened twice through the REAL
// open path (`from_factory` runs recovery + cold certification). Graceful
// shutdown validates persisted state; it is not a power-cut simulation.
#[tokio::test(flavor = "current_thread")]
async fn follower_batch_crash_points_certify_only_the_in_flight_batch() {
    for (tag, phase, record_landed, expected_versions) in [
        ("history-written", Phase::HistoryWritten, false, vec![1u64]),
        (
            "journal-confirmed",
            Phase::JournalConfirmed,
            false,
            vec![1, 2],
        ),
        (
            "journal-flushed",
            Phase::AfterJournalFlush,
            false,
            vec![1, 2],
        ),
        (
            "bookmark-persisted",
            Phase::BookmarkPersisted,
            true,
            vec![1, 2],
        ),
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
        let hook = Arc::new(test_seam::Hook::default());
        *hook.park_at.lock().unwrap() = Some(phase);
        test_seam::install(&name, Arc::clone(&hook));
        let second_batch = [replicated(2, 2)];
        let mut apply = Box::pin(apply_replicated_batch(
            &repo,
            &second_batch,
            1,
            ApplyPolicy::Trusted,
        ));
        let reached = hook.reached.notified();
        tokio::pin!(reached);
        reached.as_mut().enable();
        tokio::select! {
            _ = &mut reached => {}
            result = &mut apply => {
                panic!("apply completed before reaching {phase:?}: {result:?}")
            }
        }
        drop(apply);
        assert!(repo.tx_gate().await.expect("gate").requires_reopen());
        let record_j = {
            let info = repo.tx_info_store().await.expect("tx info store");
            let horizon = load_clean_horizon(&info).await.expect("record horizon");
            drop(info);
            horizon.expect("first batch persisted a known J")
        };
        test_seam::remove(&name);
        close(repo).await;
        for _ in 0..2 {
            let reopened = open(temp.path(), &name).await;
            let gate = reopened.tx_gate().await.expect("gate");
            let floor = gate.version_allocation_high_water_mark();
            drop(gate);
            let feed = reopened.changefeed().await.expect("changefeed").feed;
            // The GAP after reopen is exactly the in-flight batch — or none
            // when the batch's own J+bookmark record landed.
            let expected_gap = if record_landed {
                None
            } else {
                assert!(floor > record_j, "in-flight batch must raise the floor");
                Some((record_j + 1, floor))
            };
            assert_eq!(
                feed.certified_gap_interval(),
                expected_gap,
                "cold-start gap at {tag}"
            );
            let page = reopened.read_changelog_from(1, 8).await.expect("pull page");
            // The journal reader serves every event present up to the
            // confirmed bound and attaches gap_at; it never truncates the
            // page at the gap. At JournalConfirmed/AfterJournalFlush the
            // interrupted batch's event IS present (the barrier confirmed and
            // the writer flushed it) but remains uncertified inside the GAP;
            // at HistoryWritten it was never enqueued, so only the completed
            // prefix is served.
            let served_versions: Vec<u64> = page
                .events
                .iter()
                .map(|event| event.commit_version)
                .collect();
            assert_eq!(served_versions, expected_versions, "pull events at {tag}");
            let expected_page_gap = if record_landed {
                None
            } else {
                Some(record_j + 1)
            };
            assert_eq!(page.gap_at, expected_page_gap, "pull gap at {tag}");
            drop(feed);
            close(reopened).await;
        }
    }
}

// An OLD record without a known J (legacy u64 / v1) keeps today's
// conservative whole-window GAP. Regression guard: must stay true.
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
