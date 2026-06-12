use crate::repo_wal_manager::RepoWalManager;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalActiveKey, WalEntry, WalEntryV2, WalOp, WalOpV2};
use std::sync::Arc;

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn make_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn make_manager(store: &Arc<dyn Store>) -> RepoWalManager {
    RepoWalManager::new(store.clone(), 1000)
}

fn simple_entry(txn_id: u64) -> WalEntryV2 {
    WalEntryV2::new(
        txn_id,
        0,
        vec![shamir_wal::WalOpV2::Put {
            table_id_interned: 0,
            rid: rid(1),
            body: Bytes::from_static(b"hello"),
        }],
    )
}

#[tokio::test]
async fn begin_commit_no_inflight() {
    let store = make_store();
    let mgr = make_manager(&store);
    let entry = simple_entry(100);

    mgr.begin(entry).await.unwrap();
    let inflight = mgr.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(inflight[0].txn_id, 100);

    mgr.commit(100).await.unwrap();
    let inflight = mgr.list_inflight().await.unwrap();
    assert!(inflight.is_empty(), "commit must remove the marker");
}

#[tokio::test]
async fn begin_without_commit_survives_reopen() {
    let store = make_store();
    let mgr1 = make_manager(&store);
    let entry = simple_entry(200);

    mgr1.begin(entry.clone()).await.unwrap();

    let mgr2 = make_manager(&store);
    let inflight = mgr2.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(inflight[0].txn_id, 200);
    assert_eq!(inflight[0].ops.len(), 1);
}

#[tokio::test]
async fn commit_is_idempotent() {
    let store = make_store();
    let mgr = make_manager(&store);
    mgr.begin(simple_entry(300)).await.unwrap();

    mgr.commit(300).await.unwrap();
    mgr.commit(300).await.unwrap();

    let inflight = mgr.list_inflight().await.unwrap();
    assert!(inflight.is_empty());
}

#[tokio::test]
async fn commit_async_removes_marker() {
    let store = make_store();
    let mgr = make_manager(&store);
    mgr.begin(simple_entry(400)).await.unwrap();
    assert_eq!(mgr.list_inflight().await.unwrap().len(), 1);

    let handle = mgr.commit_async(400);
    handle.await.unwrap().unwrap();

    let inflight = mgr.list_inflight().await.unwrap();
    assert!(
        inflight.is_empty(),
        "commit_async must remove the marker (got {} inflight)",
        inflight.len()
    );
}

#[tokio::test]
async fn list_inflight_skips_v1_entries() {
    let store = make_store();
    let mgr = make_manager(&store);

    // Write a V1 entry manually (as per-table WalManager would).
    let v1_entry = WalEntry::new(
        10,
        vec![WalOp::RecordCreated {
            record_id: RecordId::new(),
        }],
    );
    let v1_bytes = bincode::serialize(&v1_entry).expect("v1 serialize");
    store
        .set(WalActiveKey::new(10).to_bytes(), Bytes::from(v1_bytes))
        .await
        .unwrap();

    // Write a V2 entry through RepoWalManager.
    mgr.begin(simple_entry(500)).await.unwrap();

    let inflight = mgr.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1, "should only see the V2 entry");
    assert_eq!(inflight[0].txn_id, 500);
}

#[tokio::test]
async fn fresh_txn_ids_monotonic() {
    let store = make_store();
    let mgr = make_manager(&store);
    let a = mgr.fresh_txn_id();
    let b = mgr.fresh_txn_id();
    let c = mgr.fresh_txn_id();
    assert!(a < b, "{a} should be < {b}");
    assert!(b < c, "{b} should be < {c}");
}

#[tokio::test]
async fn seed_floor_raises_counter_and_is_monotonic() {
    let store = make_store();
    // Constructor seed = 1000 (see `make_manager`).
    let mgr = make_manager(&store);

    // A floor below the current seed is a no-op: the counter does not
    // rewind, and the next id stays at the seed.
    assert_eq!(mgr.seed_floor_at_least(10), 1000);
    assert_eq!(mgr.fresh_txn_id(), 1000);

    // A floor above the current value raises it; the next id clears it.
    assert_eq!(mgr.seed_floor_at_least(5000), 5000);
    let next = mgr.fresh_txn_id();
    assert_eq!(
        next, 5000,
        "next id must equal the raised floor, got {next}"
    );
    assert!(
        mgr.fresh_txn_id() > 5000,
        "subsequent ids stay strictly above the floor"
    );
}

#[tokio::test]
async fn begin_multiple_then_list() {
    let store = make_store();
    let mgr = make_manager(&store);

    mgr.begin(simple_entry(600)).await.unwrap();
    mgr.begin(simple_entry(601)).await.unwrap();
    mgr.begin(simple_entry(602)).await.unwrap();

    let mut ids: Vec<u64> = mgr
        .list_inflight()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.txn_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![600, 601, 602]);

    mgr.commit(601).await.unwrap();
    let mut ids: Vec<u64> = mgr
        .list_inflight()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.txn_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![600, 602]);

    mgr.commit(600).await.unwrap();
    mgr.commit(602).await.unwrap();
    assert!(mgr.list_inflight().await.unwrap().is_empty());
}

#[tokio::test]
async fn recovery_round_trip_all_op_variants() {
    let store = make_store();
    let mgr = make_manager(&store);

    let entry = WalEntryV2 {
        txn_id: 999,
        repo_id_interned: 42,
        started_at_ns: 1_000_000,
        commit_version: 17,
        ops: vec![
            shamir_wal::WalOpV2::Put {
                table_id_interned: 0,
                rid: rid(1),
                body: Bytes::from_static(b"record-body"),
            },
            shamir_wal::WalOpV2::Delete {
                table_id_interned: 0,
                rid: rid(2),
            },
            shamir_wal::WalOpV2::IndexPut {
                table_id_interned: 7,
                idx_id: 11,
                key: Bytes::from_static(b"idx-key"),
                value: Bytes::from_static(b"idx-val"),
            },
            shamir_wal::WalOpV2::IndexDel {
                table_id_interned: 7,
                idx_id: 11,
                key: Bytes::from_static(b"idx-key-del"),
            },
            shamir_wal::WalOpV2::InternerOverlayMerge {
                entries: vec![(100, "email".into()), (101, "score".into())],
            },
            shamir_wal::WalOpV2::CounterDelta {
                table_id_interned: 5,
                delta: -3,
            },
        ],
    };

    mgr.begin(entry.clone()).await.unwrap();

    let inflight = mgr.list_inflight().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(
        inflight[0], entry,
        "round-tripped entry must match original"
    );
}

#[tokio::test]
async fn begin_many_round_trip() {
    let store = make_store();
    let mgr = make_manager(&store);

    let entries: Vec<WalEntryV2> = (0..5)
        .map(|i| {
            WalEntryV2::new(
                700 + i,
                0,
                vec![WalOpV2::Put {
                    table_id_interned: 0,
                    rid: rid(i as u8 + 1),
                    body: Bytes::from(format!("body-{i}")),
                }],
            )
        })
        .collect();

    mgr.begin_many(&entries).await.unwrap();

    let mut inflight = mgr.list_inflight().await.unwrap();
    inflight.sort_by_key(|e| e.txn_id);
    assert_eq!(inflight.len(), 5);
    for (i, entry) in inflight.iter().enumerate() {
        assert_eq!(entry.txn_id, 700 + i as u64);
        assert_eq!(entry.ops.len(), 1);
        assert_eq!(entries[i], *entry, "entry {i} must round-trip identically");
    }

    // Commit two, verify remaining three.
    mgr.commit(701).await.unwrap();
    mgr.commit(703).await.unwrap();
    let mut ids: Vec<u64> = mgr
        .list_inflight()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.txn_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![700, 702, 704]);
}

#[tokio::test]
async fn begin_many_empty_is_noop() {
    let store = make_store();
    let mgr = make_manager(&store);
    mgr.begin_many(&[]).await.unwrap();
    assert!(mgr.list_inflight().await.unwrap().is_empty());
}
