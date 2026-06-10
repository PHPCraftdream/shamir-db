use crate::staging_store::{StagedKind, StagingStore};
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn get_after_set_returns_staged_value() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1");
    staging.set(k.clone(), Bytes::from_static(b"v1")).await;
    assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"v1"));
}

#[tokio::test]
async fn get_after_remove_returns_not_found_even_if_base_has_key() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1");
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    staging.remove(k.clone()).await;
    assert!(staging.get(k).await.is_err());
}

#[tokio::test]
async fn get_falls_through_to_base_if_not_staged() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1");
    base.set(k.clone(), Bytes::from_static(b"from_base"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    assert_eq!(
        staging.get(k).await.unwrap(),
        Bytes::from_static(b"from_base")
    );
}

#[tokio::test]
async fn set_then_remove_collapses_to_remove() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1");

    staging.set(k.clone(), Bytes::from_static(b"v")).await;
    staging.remove(k.clone()).await;

    assert!(staging.get(k).await.is_err());
    assert_eq!(staging.len(), 1); // one key, final op = Remove
}

#[tokio::test]
async fn remove_then_set_collapses_to_set() {
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1");
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    staging.remove(k.clone()).await;
    staging.set(k.clone(), Bytes::from_static(b"new")).await;

    assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"new"));
}

#[tokio::test]
async fn drain_produces_kvop_batch() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k1: RecordKey = Bytes::from_static(b"k1");
    let k2: RecordKey = Bytes::from_static(b"k2");
    let k3: RecordKey = Bytes::from_static(b"k3");

    staging.set(k1.clone(), Bytes::from_static(b"v1")).await;
    staging.remove(k2.clone()).await;
    staging.set(k3.clone(), Bytes::from_static(b"v3")).await;

    let ops = staging.drain();
    assert_eq!(ops.len(), 3);

    let sets: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, KvOp::Set(_, _)))
        .collect();
    let removes: Vec<_> = ops
        .iter()
        .filter(|o| matches!(o, KvOp::Remove(_)))
        .collect();
    assert_eq!(sets.len(), 2);
    assert_eq!(removes.len(), 1);
}

#[tokio::test]
async fn len_tracks_unique_keys() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1");

    assert!(staging.is_empty());
    staging.set(k.clone(), Bytes::from_static(b"v1")).await;
    assert_eq!(staging.len(), 1);
    staging.set(k.clone(), Bytes::from_static(b"v2")).await;
    assert_eq!(staging.len(), 1); // same key, still 1
}

#[tokio::test]
async fn staged_op_returns_set_for_staged_value() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1");
    staging.set(k.clone(), Bytes::from_static(b"v1")).await;

    assert_eq!(
        staging.staged_op(k.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"v1")))
    );
}

#[tokio::test]
async fn staged_op_returns_removed_for_staged_remove() {
    // Even when the base store has the key, a staged Remove reports
    // Removed (and never consults the base — that is `get`'s job).
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1");
    base.set(k.clone(), Bytes::from_static(b"original"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    staging.remove(k.clone()).await;

    assert_eq!(staging.staged_op(k.as_ref()), Some(StagedKind::Removed));
}

#[tokio::test]
async fn staged_op_returns_none_when_not_staged_even_if_base_has_key() {
    // `staged_op` reports ONLY this tx's staging; a key that lives only
    // in the base is `None` (no fall-through), unlike `get`.
    let base = mem_store();
    let k: RecordKey = Bytes::from_static(b"k1");
    base.set(k.clone(), Bytes::from_static(b"from_base"))
        .await
        .unwrap();

    let staging = StagingStore::new(base);
    assert_eq!(staging.staged_op(k.as_ref()), None);
}

#[tokio::test]
async fn staged_op_reflects_last_write_wins() {
    let base = mem_store();
    let staging = StagingStore::new(base);
    let k: RecordKey = Bytes::from_static(b"k1");

    staging.set(k.clone(), Bytes::from_static(b"v")).await;
    staging.remove(k.clone()).await;
    assert_eq!(staging.staged_op(k.as_ref()), Some(StagedKind::Removed));

    staging.set(k.clone(), Bytes::from_static(b"again")).await;
    assert_eq!(
        staging.staged_op(k.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"again")))
    );
}

#[tokio::test]
async fn staged_op_borrow_probe_matches_arbitrary_length_keys() {
    // The probe takes `&[u8]`; it must find staged entries regardless of
    // key length (NOT restricted to 16-byte record ids) and allocate no
    // `Bytes` to look up.
    let base = mem_store();
    let staging = StagingStore::new(base);

    let short: RecordKey = Bytes::from_static(b"ab"); // 2 bytes
    let long: RecordKey = Bytes::from_static(b"this-key-is-forty-bytes-long-padding-here"); // > 16
    let empty: RecordKey = Bytes::from_static(b""); // 0 bytes

    staging.set(short.clone(), Bytes::from_static(b"s")).await;
    staging.remove(long.clone()).await;
    staging.set(empty.clone(), Bytes::from_static(b"e")).await;

    assert_eq!(
        staging.staged_op(short.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"s")))
    );
    assert_eq!(staging.staged_op(long.as_ref()), Some(StagedKind::Removed));
    assert_eq!(
        staging.staged_op(empty.as_ref()),
        Some(StagedKind::Set(Bytes::from_static(b"e")))
    );
    // A never-staged key of yet another length is still None.
    assert_eq!(staging.staged_op(b"never-staged-key".as_ref()), None);
}

#[tokio::test]
async fn staged_bytes_sums_keys_and_values() {
    let base = mem_store();
    let staging = StagingStore::new(base);

    // Empty staging → 0 bytes.
    assert_eq!(staging.staged_bytes(), 0);

    // One Set("ab", "12345") → key 2 + value 5 = 7 bytes.
    staging
        .set(Bytes::from_static(b"ab"), Bytes::from_static(b"12345"))
        .await;
    assert_eq!(staging.staged_bytes(), 7);

    // Add Remove("xyz") → key 3 bytes. Total = 7 + 3 = 10.
    staging.remove(Bytes::from_static(b"xyz")).await;
    assert_eq!(staging.staged_bytes(), 10);
}

#[tokio::test]
async fn snapshot_ops_does_not_consume() {
    let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(base);
    staging
        .set(
            RecordKey::from(Bytes::from_static(b"k1")),
            Bytes::from_static(b"v1"),
        )
        .await;
    staging
        .remove(RecordKey::from(Bytes::from_static(b"k2")))
        .await;

    let snapshot1 = staging.snapshot_ops();
    let snapshot2 = staging.snapshot_ops();
    assert_eq!(snapshot1.len(), 2);
    assert_eq!(snapshot2.len(), 2, "snapshot_ops must NOT consume");
    assert_eq!(staging.len(), 2);
}
