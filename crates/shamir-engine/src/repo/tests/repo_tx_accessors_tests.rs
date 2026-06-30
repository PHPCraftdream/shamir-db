use std::sync::Arc;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn make_repo() -> RepoInstance {
    create_test_instance()
}

#[tokio::test]
async fn tx_gate_initializes_lazy() {
    let instance = create_test_instance();
    let gate1 = instance.tx_gate().await.unwrap();
    let gate2 = instance.tx_gate().await.unwrap();
    assert!(
        Arc::ptr_eq(&gate1, &gate2),
        "tx_gate must return the same Arc on repeated calls"
    );
}

#[tokio::test]
async fn repo_wal_initializes_lazy() {
    let instance = create_test_instance();
    let wal1 = instance.repo_wal().await.unwrap();
    let wal2 = instance.repo_wal().await.unwrap();
    assert!(
        Arc::ptr_eq(&wal1, &wal2),
        "repo_wal must return the same Arc on repeated calls"
    );
}

/// txn_id recovery floor (mirror of the CRIT-B version-floor test
/// `recovery_advances_gate_past_replayed_commit_version`): an inflight V2
/// WAL entry left by a crash carries a txn_id the periodic `NextTxId`
/// snapshot may not yet cover. After a "restart" the freshly-seeded
/// `RepoWalManager` must hand out an id strictly greater than that inflight
/// txn_id, never reusing it.
#[tokio::test]
async fn repo_wal_seeds_txn_id_floor_above_inflight() {
    use crate::repo::BoxRepoFactory;
    use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

    // F5e: the single WAL write path uses a per-instance `Mem` sink for
    // in-memory repos, so an injected inflight entry only genuinely survives a
    // restart on the file segment — this test is therefore disk-backed.
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let path = tempdir.path().to_path_buf();

    async fn open_disk(path: &std::path::Path) -> RepoInstance {
        let mut last_err = None;
        for _attempt in 0..10 {
            match RepoInstance::from_factory(
                "r".into(),
                BoxRepoFactory::fjall_raw(path.to_path_buf()),
                vec![TableConfig::new("t")],
            )
            .await
            {
                Ok(r) => return r,
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
        panic!("open_disk failed after 10 retries: {last_err:?}");
    }

    // Seed a high inflight txn_id WITHOUT committing, as a crash between commit
    // Phase 4 and Phase 7 leaves behind. Use a throwaway RepoInstance over the
    // disk path and drop it (models the in-memory side of a restart; the entry
    // survives in the file segment).
    const HIGH_TXN_ID: u64 = 5_000;
    {
        let seed = open_disk(&path).await;
        let wal = seed.repo_wal().await.unwrap();
        let entry = WalEntryV2::new(
            HIGH_TXN_ID,
            crate::repo::repo_instance::repo_token(seed.name()),
            vec![WalOpV2::Put {
                table_id_interned: crate::table::table_manager::table_token_for("t"),
                rid: {
                    let mut a = [0u8; 16];
                    a[15] = 1;
                    shamir_types::types::record_id::RecordId(a)
                },
                body: bytes::Bytes::from_static(b"x"),
            }],
        );
        wal.begin_grouped(&entry, WalDurability::Synced)
            .await
            .unwrap();
        drop(seed);
    }

    // === SIMULATED RESTART: fresh RepoInstance over the same path ===
    let repo = open_disk(&path).await;

    // The inflight entry survived the "restart".
    let wal = repo.repo_wal().await.unwrap();
    assert_eq!(wal.recover().await.unwrap().len(), 1);

    // The decisive assertion: the next id must clear the inflight txn_id —
    // no reuse. (The persisted `NextTxId` snapshot was never written here, so
    // without the inflight pre-scan the seed would default to 1 and collide.)
    let next = wal.fresh_txn_id();
    assert!(
        next > HIGH_TXN_ID,
        "fresh_txn_id must exceed the inflight txn_id ({HIGH_TXN_ID}), got {next}"
    );
}

#[tokio::test]
async fn tx_gate_and_wal_share_info_store_via_repo() {
    let instance = create_test_instance();
    let _gate = instance.tx_gate().await.unwrap();
    let _wal = instance.repo_wal().await.unwrap();

    let gate2 = instance.tx_gate().await.unwrap();
    let wal2 = instance.repo_wal().await.unwrap();
    let gate3 = instance.tx_gate().await.unwrap();
    let wal3 = instance.repo_wal().await.unwrap();

    assert!(Arc::ptr_eq(&gate2, &gate3));
    assert!(Arc::ptr_eq(&wal2, &wal3));
}

#[test]
fn repo_name_stored() {
    let repo = create_test_instance();
    assert_eq!(repo.name(), "test");
}

#[test]
fn repo_token_deterministic() {
    let t1 = crate::repo::repo_instance::repo_token("my_repo");
    let t2 = crate::repo::repo_instance::repo_token("my_repo");
    assert_eq!(t1, t2);
    assert_ne!(t1, 0);
}

#[tokio::test]
async fn create_table_context_populates_per_table_mvcc() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let _ = repo.get_table("users").await.unwrap();

    let token = crate::table::table_manager::table_token_for("users");

    let (tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    assert!(
        tx.version_provider.is_some(),
        "Serializable tx must have provider"
    );

    let v = tx
        .version_provider
        .as_ref()
        .unwrap()
        .version_of(token, &bytes::Bytes::from_static(b"unknown"));
    assert_eq!(v, Some(0), "unknown key returns version 0");
}

#[tokio::test]
async fn begin_tx_snapshot_does_not_attach_provider() {
    let repo = make_repo();
    let (tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    assert!(
        tx.version_provider.is_none(),
        "SI tx must not have provider"
    );
}

#[tokio::test]
async fn version_provider_returns_none_for_unknown_table() {
    let repo = make_repo();
    // Don't create any tables — per_table_mvcc is empty.
    let (mut tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Serializable)
        .await
        .unwrap();
    // Record a read from an unknown table.
    tx.record_read(12345, bytes::Bytes::from_static(b"k"), 5);

    let result = repo.commit_tx(tx).await;
    assert!(
        result.is_err(),
        "unknown table in read_set must cause conflict"
    );
}
