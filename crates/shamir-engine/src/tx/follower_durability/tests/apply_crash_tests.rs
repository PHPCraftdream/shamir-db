#[cfg(feature = "fjall")]
use super::super::test_seam::{self, Phase};
#[cfg(feature = "fjall")]
use futures::StreamExt;
#[cfg(feature = "fjall")]
use shamir_storage::types::RecordKey;
#[cfg(feature = "fjall")]
use shamir_tx::{ChangeOp, ChangelogEvent, RecordChange};
#[cfg(feature = "fjall")]
use shamir_types::access::Actor;
#[cfg(feature = "fjall")]
use shamir_types::types::record_id::RecordId;
#[cfg(feature = "fjall")]
use shamir_types::types::value::InnerValue;
#[cfg(feature = "fjall")]
use std::path::Path;
#[cfg(feature = "fjall")]
use std::sync::Arc;

#[cfg(feature = "fjall")]
use crate::meta::repl_bookmark::{load_clean_horizon, load_replication_bookmark};
#[cfg(feature = "fjall")]
use crate::repo::{BoxRepoFactory, RepoInstance};
#[cfg(feature = "fjall")]
use crate::table::TableConfig;
#[cfg(feature = "fjall")]
use crate::tx::apply_replicated::{apply_replicated_batch, follower_durability::in_flight_range};
#[cfg(feature = "fjall")]
use crate::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};

#[cfg(feature = "fjall")]
fn event(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([id; 16]).to_bytes(),
                op: ChangeOp::Put,
                value: Some(InnerValue::Str(format!("row-{id}")).to_bytes().unwrap()),
            }],
        },
        field_bindings: Vec::new(),
    }
}

#[cfg(feature = "fjall")]
async fn open(path: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(path),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap()
}

#[cfg(feature = "fjall")]
async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
}

#[cfg(feature = "fjall")]
fn repo_name(tag: &str, path: &Path) -> String {
    format!("{tag}-{}", path.display().to_string().replace('\\', "_"))
}

#[cfg(feature = "fjall")]
async fn journal(repo: &RepoInstance, version: u64) -> bytes::Bytes {
    repo.follower_journal_store()
        .await
        .unwrap()
        .get(RecordKey::from_slice(&version.to_be_bytes()))
        .await
        .unwrap()
}

#[cfg(feature = "fjall")]
async fn journal_entries(repo: &RepoInstance) -> Vec<(RecordKey, bytes::Bytes)> {
    repo.follower_journal_store()
        .await
        .unwrap()
        .iter_stream(128)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .collect()
}

// Graceful shutdown/reopen validates persisted state; it is not a power-cut simulation.
#[cfg(feature = "fjall")]
#[tokio::test]
async fn real_fjall_crash_phase_matrix() {
    for (tag, phase) in [
        ("history-written", Phase::HistoryWritten),
        ("journal-confirmed", Phase::JournalConfirmed),
        ("journal-flushed", Phase::AfterJournalFlush),
        ("bookmark-persisted", Phase::BookmarkPersisted),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let name = repo_name(tag, temp.path());
        let repo = open(temp.path(), &name).await;
        assert_eq!(
            apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
                .await
                .unwrap(),
            ApplyOutcome::Applied { local_version: 1 }
        );
        let hook = Arc::new(test_seam::Hook::default());
        *hook.park_at.lock().unwrap() = Some(phase);
        test_seam::install(&name, Arc::clone(&hook));
        let events = [event(2, 2)];
        let mut apply = Box::pin(apply_replicated_batch(
            &repo,
            &events,
            1,
            ApplyPolicy::Trusted,
        ));
        let reached = hook.reached.notified();
        tokio::pin!(reached);
        reached.as_mut().enable();
        tokio::select! {
            _ = &mut reached => {},
            result = &mut apply => panic!("apply completed before reaching {phase:?}: {result:?}"),
        }
        drop(apply);
        assert!(repo.tx_gate().await.unwrap().requires_reopen());
        let (bookmark, horizon, floor, first_event) = {
            let info = repo.tx_info_store().await.unwrap();
            let gate = repo.tx_gate().await.unwrap();
            let facts = (
                load_replication_bookmark(&info).await.unwrap(),
                load_clean_horizon(&info).await.unwrap(),
                gate.version_allocation_high_water_mark(),
                journal(&repo, 1).await,
            );
            drop(gate);
            drop(info);
            facts
        };
        test_seam::remove(&name);
        close(repo).await;
        for _ in 0..2 {
            let reopened = open(temp.path(), &name).await;
            let info = reopened.tx_info_store().await.unwrap();
            assert_eq!(load_replication_bookmark(&info).await.unwrap(), bookmark);
            assert_eq!(load_clean_horizon(&info).await.unwrap(), horizon);
            let reopened_gate = reopened.tx_gate().await.unwrap();
            let reopened_floor = reopened_gate.version_allocation_high_water_mark();
            assert_eq!(reopened_floor, floor);
            assert_eq!(
                in_flight_range(horizon, reopened_floor),
                if bookmark == 1 { Some(2..=2) } else { None }
            );
            assert_eq!(journal(&reopened, 1).await, first_event);
            let entries = journal_entries(&reopened).await;
            let decoded: Vec<ChangelogEvent> = entries
                .iter()
                .filter(|(key, _)| key.as_ref().len() == 8)
                .map(|(_, value)| rmp_serde::from_slice(value).unwrap())
                .collect();
            assert_eq!(
                decoded
                    .iter()
                    .filter(|event| event.commit_version == 1)
                    .count(),
                1
            );
            if matches!(
                phase,
                Phase::JournalConfirmed | Phase::AfterJournalFlush | Phase::BookmarkPersisted
            ) {
                let mut expected_two = event(2, 2).event;
                expected_two.repo = name.clone();
                expected_two.commit_version = 2;
                let expected_two = bytes::Bytes::from(rmp_serde::to_vec(&expected_two).unwrap());
                assert_eq!(
                    decoded
                        .iter()
                        .filter(|event| event.commit_version == 2)
                        .count(),
                    1
                );
                assert_eq!(journal(&reopened, 2).await, expected_two);
            } else {
                assert_eq!(
                    decoded
                        .iter()
                        .filter(|event| event.commit_version == 2)
                        .count(),
                    0
                );
            }
            let table = reopened.get_table("items").await.unwrap();
            assert!(table.get_bytes(RecordId([1; 16])).await.unwrap().is_some());
            if matches!(phase, Phase::AfterJournalFlush | Phase::BookmarkPersisted) {
                assert!(table.get_bytes(RecordId([2; 16])).await.unwrap().is_some());
            }
            drop(table);
            if phase == Phase::BookmarkPersisted {
                let before_one = journal(&reopened, 1).await;
                let before_two = journal(&reopened, 2).await;
                let floor_before = reopened_gate.version_allocation_high_water_mark();
                assert_eq!(
                    apply_replicated_batch(&reopened, &[event(2, 2)], 2, ApplyPolicy::Trusted)
                        .await
                        .unwrap(),
                    ApplyOutcome::Skipped
                );
                assert_eq!(
                    reopened_gate.version_allocation_high_water_mark(),
                    floor_before
                );
                assert_eq!(journal(&reopened, 1).await, before_one);
                assert_eq!(journal(&reopened, 2).await, before_two);
            }
            drop(info);
            drop(reopened_gate);
            close(reopened).await;
        }
        assert_eq!(
            bookmark,
            if phase == Phase::BookmarkPersisted {
                2
            } else {
                1
            }
        );
        assert_eq!(
            horizon,
            if phase == Phase::BookmarkPersisted {
                Some(2)
            } else {
                Some(1)
            }
        );
        if phase == Phase::AfterJournalFlush {
            assert!(floor >= 2);
        }
    }
}
