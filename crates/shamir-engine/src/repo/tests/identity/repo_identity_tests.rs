use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::Barrier;

use crate::meta::repo_identity::RepoIdentity;
#[cfg(feature = "fjall")]
use crate::meta::repo_identity::{load_state, LifecycleState};
use crate::meta::{MetaEnvelope, MetaKey};
#[cfg(feature = "fjall")]
use crate::repo::durability_policy::{
    DurabilityPolicy, ReplicationServe, RetentionOverflow, WalCommitTier,
    DEFAULT_WAL_RETENTION_MAX_BYTES,
};
#[cfg(feature = "fjall")]
use crate::repo::BoxRepoFactory;
use crate::repo::{BoxRepo, RepoInstance};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::RecordKey;

fn handle(backing: &Arc<InMemoryRepo>) -> RepoInstance {
    RepoInstance::new("main".into(), BoxRepo::InMemory(backing.clone()), vec![])
}

#[tokio::test]
async fn legacy_reopen_and_recreation_identity() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = handle(&backing);
    let identity = repo.repo_identity().await.unwrap();
    assert_eq!(identity.history_epoch, 0);
    drop(repo);
    assert_eq!(handle(&backing).repo_identity().await.unwrap(), identity);
    let recreated = handle(&Arc::new(InMemoryRepo::new()));
    assert_ne!(
        recreated.repo_identity().await.unwrap().lineage_id,
        identity.lineage_id
    );
}

#[tokio::test]
async fn cloned_handles_serialize_create_and_bump() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = handle(&backing);
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let repo = repo.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.bump_history_epoch().await.unwrap()
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_eq!(first.lineage_id, second.lineage_id);
    assert_eq!(repo.repo_identity().await.unwrap().history_epoch, 2);
}

#[tokio::test]
async fn overflow_refused_without_identity_change() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = handle(&backing);
    let identity = RepoIdentity {
        lineage_id: [7; 16],
        history_epoch: u64::MAX,
    };
    RepoIdentity::save_record(&repo.tx_info_store().await.unwrap(), identity, None)
        .await
        .unwrap();
    assert!(repo
        .bump_history_epoch()
        .await
        .unwrap_err()
        .to_string()
        .contains("overflow"));
    assert_eq!(repo.repo_identity().await.unwrap(), identity);
}

#[tokio::test]
async fn restore_receipt_survives_reopen_and_new_operation_bumps() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = handle(&backing);
    let original = repo.repo_identity().await.unwrap();
    let bumped = repo.apply_restore_operation([1; 16]).await.unwrap();
    assert_eq!(bumped.history_epoch, 1);
    assert_eq!(bumped.lineage_id, original.lineage_id);
    drop(repo);
    let reopened = handle(&backing);
    assert_eq!(
        reopened.apply_restore_operation([1; 16]).await.unwrap(),
        bumped
    );
    assert_eq!(
        reopened
            .apply_restore_operation([2; 16])
            .await
            .unwrap()
            .history_epoch,
        2
    );
}

#[cfg(feature = "fjall")]
#[test]
fn identity_survives_disk_reopen_and_corruption_refuses_open() {
    // Destroy the complete runtime between opens, not a timed lock retry.
    let directory = tempfile::TempDir::new().unwrap();
    let open = || {
        RepoInstance::from_factory(
            "main".into(),
            BoxRepoFactory::fjall_raw(directory.path()),
            vec![],
        )
    };
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let identity = runtime().block_on(async {
        let repo = open().await.unwrap();
        repo.repo_identity().await.unwrap()
    });
    runtime().block_on(async {
        let repo = open().await.unwrap();
        assert_eq!(repo.repo_identity().await.unwrap(), identity);
        let store = repo.tx_info_store().await.unwrap();
        store
            .set(
                RecordKey::from_slice(MetaKey::RepoIdentity.as_record_id().as_bytes()),
                Bytes::from_static(b"corrupt"),
            )
            .await
            .unwrap();
        store.flush().await.unwrap();
    });
    runtime().block_on(async {
        let error = open().await.err().expect("corrupt identity must fail open");
        assert!(error.to_string().contains("identity"));
    });
}

#[tokio::test]
async fn identity_version_length_and_receipt_roundtrip_validation() {
    let backing = Arc::new(InMemoryRepo::new());
    let repo = handle(&backing);
    let store = repo.tx_info_store().await.unwrap();
    for epoch in [0, 1, u64::MAX] {
        for receipt in [None, Some([9; 16])] {
            let identity = RepoIdentity {
                lineage_id: [255; 16],
                history_epoch: epoch,
            };
            RepoIdentity::save_record(&store, identity, receipt)
                .await
                .unwrap();
            assert_eq!(repo.repo_identity().await.unwrap(), identity);
        }
    }
    let identity = RepoIdentity {
        lineage_id: [1; 16],
        history_epoch: 0,
    };
    let valid = MetaEnvelope::new((1u16, identity, None::<[u8; 16]>))
        .encode()
        .unwrap();
    let mut bad_magic = valid.clone();
    bad_magic[0] ^= 1;
    let mut trailing = valid.clone();
    trailing.push(0);
    let unsupported = MetaEnvelope::new((2u16, identity, None::<[u8; 16]>))
        .encode()
        .unwrap();
    for bytes in [
        vec![],
        bad_magic,
        trailing,
        unsupported,
        valid[..40].to_vec(),
    ] {
        store
            .set(
                RecordKey::from_slice(MetaKey::RepoIdentity.as_record_id().as_bytes()),
                Bytes::from(bytes),
            )
            .await
            .unwrap();
        assert!(
            repo.repo_identity().await.is_err(),
            "corrupt record must not default"
        );
    }
}

/// Switching a repo with an ARMED lifecycle record to `durable` serving must
/// NOT erase the obligation (no Clean certificate is written under durable
/// mode), and the next open under `completed_reseed` serving then costs the
/// history-epoch bump exactly once — lineage unchanged.
#[cfg(feature = "fjall")]
#[test]
fn armed_record_survives_mode_switch_and_forces_bump_on_next_fast_open() {
    let directory = tempfile::TempDir::new().unwrap();
    let policy = |serve| {
        DurabilityPolicy::new(
            WalCommitTier::Buffered,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::RefuseWrites,
            serve,
        )
    };
    let open_reseed = || {
        RepoInstance::from_factory_with_durability(
            "main".into(),
            BoxRepoFactory::fjall_raw(directory.path()),
            vec![],
            policy(ReplicationServe::CompletedReseed),
        )
    };
    // Destroy the complete runtime between opens (Windows fjall lock),
    // exactly like `identity_survives_disk_reopen_and_corruption_refuses_open`.
    let runtime = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let before = runtime().block_on(async {
        let repo = open_reseed().await.unwrap();
        // Arm via a replication page read: under CompletedReseed serving the
        // page path arms the lifecycle record BEFORE any byte is served.
        let (_, gap) = repo.read_changelog_for_replication(0, 100).await.unwrap();
        assert_eq!(gap, None);
        let store = repo.tx_info_store().await.unwrap();
        assert_eq!(
            load_state(&store).await.unwrap(),
            Some(LifecycleState::Armed)
        );
        // Switch to Durable: the Armed record must survive the switch.
        repo.set_durability(policy(ReplicationServe::Durable))
            .unwrap();
        assert_eq!(
            load_state(&store).await.unwrap(),
            Some(LifecycleState::Armed),
            "switching to durable must not erase an armed obligation"
        );
        // Clean shutdown under Durable serving writes NO Clean certificate
        // (the certificate only closes a completed-reseed serve period), so
        // the record stays Armed even after a graceful stop.
        repo.shutdown_clean(std::time::Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            load_state(&store).await.unwrap(),
            Some(LifecycleState::Armed)
        );
        drop(store);
        repo.repo_identity().await.unwrap()
    });
    // The next fast open under CompletedReseed finds Armed with no later
    // Clean certificate and bumps the epoch exactly +1.
    runtime().block_on(async {
        let reopened = open_reseed().await.unwrap();
        let identity = reopened.repo_identity().await.unwrap();
        assert_eq!(
            identity.lineage_id, before.lineage_id,
            "the bump must NEVER change the lineage"
        );
        assert_eq!(identity.history_epoch, before.history_epoch + 1);
    });
}
