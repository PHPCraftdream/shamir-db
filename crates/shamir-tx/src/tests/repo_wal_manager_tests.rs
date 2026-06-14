use crate::repo_wal_manager::RepoWalManager;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalDurability, WalEntryV2, WalGroupCommit, WalOpV2, WalSink};
use std::sync::Arc;

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

/// Build a manager over a fresh in-RAM (`Mem`) group — the single write
/// path shared with disk repos.
fn make_manager() -> RepoWalManager {
    let group = Arc::new(WalGroupCommit::new(Arc::new(WalSink::mem())));
    RepoWalManager::new(1000, group)
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
async fn begin_then_recover() {
    let mgr = make_manager();
    let entry = simple_entry(100);

    mgr.begin_grouped(entry, WalDurability::Buffered)
        .await
        .unwrap();
    let inflight = mgr.recover().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(inflight[0].txn_id, 100);

    // commit is a no-op now (entries live in the segment until truncation);
    // recovery replays them idempotently.
    mgr.commit(100).await.unwrap();
}

#[tokio::test]
async fn commit_is_idempotent_noop() {
    let mgr = make_manager();
    mgr.begin_grouped(simple_entry(300), WalDurability::Buffered)
        .await
        .unwrap();

    mgr.commit(300).await.unwrap();
    mgr.commit(300).await.unwrap();

    // Entry remains replayable (commit does not remove it).
    assert_eq!(mgr.recover().await.unwrap().len(), 1);
}

#[tokio::test]
async fn fresh_txn_ids_monotonic() {
    let mgr = make_manager();
    let a = mgr.fresh_txn_id();
    let b = mgr.fresh_txn_id();
    let c = mgr.fresh_txn_id();
    assert!(a < b, "{a} should be < {b}");
    assert!(b < c, "{b} should be < {c}");
}

#[tokio::test]
async fn seed_floor_raises_counter_and_is_monotonic() {
    // Constructor seed = 1000 (see `make_manager`).
    let mgr = make_manager();

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
async fn begin_multiple_then_recover() {
    let mgr = make_manager();

    mgr.begin_grouped(simple_entry(600), WalDurability::Buffered)
        .await
        .unwrap();
    mgr.begin_grouped(simple_entry(601), WalDurability::Buffered)
        .await
        .unwrap();
    mgr.begin_grouped(simple_entry(602), WalDurability::Buffered)
        .await
        .unwrap();

    let mut ids: Vec<u64> = mgr
        .recover()
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.txn_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec![600, 601, 602]);
}

#[tokio::test]
async fn recovery_round_trip_all_op_variants() {
    let mgr = make_manager();

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
        interner_delta: vec![],
    };

    mgr.begin_grouped(entry.clone(), WalDurability::Buffered)
        .await
        .unwrap();

    let inflight = mgr.recover().await.unwrap();
    assert_eq!(inflight.len(), 1);
    assert_eq!(
        inflight[0], entry,
        "round-tripped entry must match original"
    );
}

#[tokio::test]
async fn begin_many_round_trip() {
    let mgr = make_manager();

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

    mgr.begin_grouped_many(&entries, WalDurability::Buffered)
        .await
        .unwrap();

    let mut inflight = mgr.recover().await.unwrap();
    inflight.sort_by_key(|e| e.txn_id);
    assert_eq!(inflight.len(), 5);
    for (i, entry) in inflight.iter().enumerate() {
        assert_eq!(entry.txn_id, 700 + i as u64);
        assert_eq!(entry.ops.len(), 1);
        assert_eq!(entries[i], *entry, "entry {i} must round-trip identically");
    }
}

#[tokio::test]
async fn begin_many_empty_is_noop() {
    let mgr = make_manager();
    mgr.begin_grouped_many(&[], WalDurability::Buffered)
        .await
        .unwrap();
    assert!(mgr.recover().await.unwrap().is_empty());
}
