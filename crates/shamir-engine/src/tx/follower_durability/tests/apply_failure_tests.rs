use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
use shamir_tx::{ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::meta::repl_bookmark::{load_clean_horizon, load_replication_bookmark};
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::{
    apply_replicated_batch, apply_replicated_batch_legacy,
    follower_durability::{in_flight_range, test_seam},
};
use crate::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};

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

async fn open(path: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(path),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap()
}

fn repo_name(tag: &str, path: &Path) -> String {
    format!("{tag}-{}", path.display().to_string().replace('\\', "_"))
}

async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
}

async fn journal_bytes(repo: &RepoInstance, version: u64) -> bytes::Bytes {
    repo.follower_journal_store()
        .await
        .unwrap()
        .get(RecordKey::from_slice(&version.to_be_bytes()))
        .await
        .unwrap()
}

#[tokio::test]
async fn failed_barrier_preserves_persisted_certificate_and_fail_stops() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("follower-barrier-fail", temp.path());
    let repo = open(temp.path(), &name).await;
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 1 },
    );
    let baseline = {
        let info = repo.tx_info_store().await.unwrap();
        (
            load_replication_bookmark(&info).await.unwrap(),
            load_clean_horizon(&info).await.unwrap(),
        )
    };
    assert_eq!(baseline, (1, Some(1)));

    let hook = Arc::new(test_seam::Hook::default());
    hook.fail_journal_barrier.store(true, Ordering::Release);
    test_seam::install(&name, hook);
    assert!(matches!(
        apply_replicated_batch_legacy(&repo, &[event(2, 2)], 1, ApplyPolicy::Trusted).await,
        Err(DbError::RepoRequiresReopen(_))
    ));
    let gate = repo.tx_gate().await.unwrap();
    assert!(gate.requires_reopen());
    assert_eq!(gate.version_allocation_high_water_mark(), 2);
    {
        let info = repo.tx_info_store().await.unwrap();
        assert_eq!(load_replication_bookmark(&info).await.unwrap(), 1);
        assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(1));
    }
    let high_water = gate.version_allocation_high_water_mark();
    assert!(matches!(
        apply_replicated_batch_legacy(&repo, &[event(2, 2)], 1, ApplyPolicy::Trusted).await,
        Err(DbError::RepoRequiresReopen(_))
    ));
    assert_eq!(gate.version_allocation_high_water_mark(), high_water);
    test_seam::remove(&name);
    drop(gate);
    close(repo).await;

    for _ in 0..2 {
        let reopened = open(temp.path(), &name).await;
        let gate = reopened.tx_gate().await.unwrap();
        let floor = gate.version_allocation_high_water_mark();
        assert_eq!(floor, 2);
        {
            let info = reopened.tx_info_store().await.unwrap();
            assert_eq!(load_replication_bookmark(&info).await.unwrap(), 1);
            assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(1));
            assert_eq!(in_flight_range(Some(1), floor), Some(2..=2));
        }
        let mut expected = event(1, 1).event;
        expected.repo = name.clone();
        assert_eq!(
            journal_bytes(&reopened, 1).await,
            bytes::Bytes::from(rmp_serde::to_vec(&expected).unwrap())
        );
        let table = reopened.get_table("items").await.unwrap();
        assert!(table.get_bytes(RecordId([1; 16])).await.unwrap().is_some());
        drop(table);
        drop(gate);
        close(reopened).await;
    }
}

#[tokio::test]
async fn bypass_negative_control_advances_ack_and_redelivery_does_not_allocate() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("follower-barrier-bypass", temp.path());
    let repo = open(temp.path(), &name).await;
    assert_eq!(
        apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 1 },
    );
    let hook = Arc::new(test_seam::Hook::default());
    hook.fail_journal_barrier.store(true, Ordering::Release);
    hook.bypass_journal_barrier.store(true, Ordering::Release);
    test_seam::install(&name, hook);
    assert_eq!(
        apply_replicated_batch(&repo, &[event(2, 2)], 1, ApplyPolicy::Trusted)
            .await
            .unwrap(),
        ApplyOutcome::Applied { local_version: 2 },
        "explicit bypass negative control must demonstrate unsafe acknowledgement",
    );
    assert_eq!(repo.replication_bookmark().await.unwrap(), 2);
    test_seam::remove(&name);
    close(repo).await;

    for _ in 0..2 {
        let reopened = open(temp.path(), &name).await;
        let info = reopened.tx_info_store().await.unwrap();
        let journal = reopened.follower_journal_store().await.unwrap();
        assert_eq!(load_replication_bookmark(&info).await.unwrap(), 2);
        assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(2));
        let before_one = journal_bytes(&reopened, 1).await;
        let before_two = journal_bytes(&reopened, 2).await;
        let gate = reopened.tx_gate().await.unwrap();
        let floor = gate.version_allocation_high_water_mark();
        assert_eq!(floor, 2);
        assert_eq!(
            apply_replicated_batch(&reopened, &[event(2, 2)], 2, ApplyPolicy::Trusted)
                .await
                .unwrap(),
            ApplyOutcome::Skipped,
        );
        assert_eq!(gate.version_allocation_high_water_mark(), floor);
        assert_eq!(load_replication_bookmark(&info).await.unwrap(), 2);
        assert_eq!(journal_bytes(&reopened, 1).await, before_one);
        assert_eq!(journal_bytes(&reopened, 2).await, before_two);
        drop(gate);
        drop(info);
        drop(journal);
        close(reopened).await;
    }
}
