use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::access::Actor;
use std::sync::Arc;

use super::super::journal::{
    delete_events_in_range, encode_event, events_in_range, flush_journal, key_version,
    put_event_if_missing, read_event,
};
use crate::changefeed::{
    clean_horizon_key, gap_marker_key, version_key, ChangeOp, ChangelogEvent, RecordChange,
};

fn event(version: u64, marker: u8) -> ChangelogEvent {
    ChangelogEvent {
        repo: "test".into(),
        commit_version: version,
        tx_id: 3,
        actor: Actor::System,
        timestamp_ns: 11,
        changes: vec![RecordChange {
            table: "items".into(),
            key: Bytes::from_static(b"record"),
            op: ChangeOp::Put,
            value: Some(Bytes::from(vec![marker])),
        }],
    }
}

fn store() -> Arc<InMemoryStore> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn put_missing_equal_mismatch_and_corrupt_refusal() {
    let store = store();
    let expected = event(4, 1);
    put_event_if_missing(store.as_ref(), &expected)
        .await
        .unwrap();
    let original = store.get(version_key(4).into()).await.unwrap();
    put_event_if_missing(store.as_ref(), &expected)
        .await
        .unwrap();
    assert_eq!(store.get(version_key(4).into()).await.unwrap(), original);
    assert!(put_event_if_missing(store.as_ref(), &event(4, 2))
        .await
        .is_err());
    assert_eq!(store.get(version_key(4).into()).await.unwrap(), original);

    store
        .set(version_key(7).into(), Bytes::from_static(b"broken"))
        .await
        .unwrap();
    assert!(put_event_if_missing(store.as_ref(), &event(7, 1))
        .await
        .is_err());
    assert!(read_event(store.as_ref(), 7).await.is_err());
}

#[tokio::test]
async fn reads_and_scans_refuse_key_version_mismatch() {
    let store = store();
    store
        .set(version_key(8).into(), encode_event(&event(9, 1)).unwrap())
        .await
        .unwrap();
    assert!(read_event(store.as_ref(), 8).await.is_err());
    assert!(events_in_range(store.as_ref(), 7, 9, 2).await.is_err());
}

#[tokio::test]
async fn bounded_range_skips_metadata_and_delete_preserves_outside_records() {
    let store = store();
    for version in [2, 3, 4, 5] {
        put_event_if_missing(store.as_ref(), &event(version, version as u8))
            .await
            .unwrap();
    }
    store
        .set(gap_marker_key().into(), Bytes::from_static(b"marker"))
        .await
        .unwrap();
    store
        .set(clean_horizon_key().into(), Bytes::from_static(b"horizon"))
        .await
        .unwrap();

    // The inclusive range contains the low-valued metadata keys as well as events.
    let range = events_in_range(store.as_ref(), 0, u64::MAX, 2)
        .await
        .unwrap();
    assert_eq!(
        range
            .iter()
            .map(|item| item.commit_version)
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 5]
    );

    delete_events_in_range(store.as_ref(), 2, 4, 2)
        .await
        .unwrap();
    flush_journal(store.as_ref()).await.unwrap();
    assert!(read_event(store.as_ref(), 3).await.unwrap().is_none());
    assert!(read_event(store.as_ref(), 4).await.unwrap().is_none());
    assert!(read_event(store.as_ref(), 2).await.unwrap().is_some());
    assert!(read_event(store.as_ref(), 5).await.unwrap().is_some());
    assert_eq!(
        store.get(gap_marker_key().into()).await.unwrap(),
        Bytes::from_static(b"marker")
    );
    assert_eq!(
        store.get(clean_horizon_key().into()).await.unwrap(),
        Bytes::from_static(b"horizon")
    );
}

#[tokio::test]
async fn bounded_scan_rejects_malformed_nonmetadata_key() {
    let store = store();
    let mut malformed = version_key(5).to_vec();
    malformed.push(0);
    store
        .set(Bytes::from(malformed).into(), Bytes::from_static(b"bad"))
        .await
        .unwrap();
    assert!(events_in_range(store.as_ref(), 0, 10, 4).await.is_err());
}

#[test]
fn gap_marker_is_excluded_by_key_classifier() {
    assert_eq!(key_version(&gap_marker_key()).unwrap(), None);
}
