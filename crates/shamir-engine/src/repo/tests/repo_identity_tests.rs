use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::Barrier;

use crate::meta::repo_identity::RepoIdentity;
use crate::meta::{MetaEnvelope, MetaKey};
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
