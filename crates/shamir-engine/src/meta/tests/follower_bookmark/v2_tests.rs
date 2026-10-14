use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};

use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark,
    save_bookmark_and_clean_horizon, save_fence_record, save_replication_bookmark, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaKey};

fn store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn bookmark_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

#[tokio::test]
async fn legacy_u64_and_v1_records_decode_with_unknown_horizon() {
    let info = store();
    let legacy = MetaEnvelope::new(23u64).encode().unwrap();
    info.set(bookmark_key(), Bytes::from(legacy)).await.unwrap();
    assert_eq!(load_replication_bookmark(&info).await.unwrap(), 23);
    assert_eq!(load_clean_horizon(&info).await.unwrap(), None);

    let fence = FenceRecord {
        bookmark: 31,
        lineage_id: [4; 16],
        history_epoch: 5,
        observed_identity: None,
    };
    let v1 = MetaEnvelope::new((1u16, fence)).encode().unwrap();
    info.set(bookmark_key(), Bytes::from(v1)).await.unwrap();
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), None);
}

#[tokio::test]
async fn v2_round_trip_and_both_save_apis_preserve_identity_and_horizon() {
    let info = store();
    let mut fence = FenceRecord {
        bookmark: 10,
        lineage_id: [8; 16],
        history_epoch: 9,
        observed_identity: None,
    };
    save_fence_record(&info, fence).await.unwrap();
    save_bookmark_and_clean_horizon(&info, 20, 18)
        .await
        .unwrap();
    fence.bookmark = 20;
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(18));

    save_replication_bookmark(&info, 25).await.unwrap();
    fence.bookmark = 25;
    assert_eq!(load_replication_bookmark(&info).await.unwrap(), 25);
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(18));

    save_bookmark_and_clean_horizon(&info, 30, 12)
        .await
        .unwrap();
    fence.bookmark = 30;
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(18));
}

#[tokio::test]
async fn identity_cursor_and_clean_horizon_updates_work_without_a_fence() {
    let info = store();
    save_bookmark_and_clean_horizon(&info, 40, 35)
        .await
        .unwrap();
    assert_eq!(load_replication_bookmark(&info).await.unwrap(), 40);
    assert_eq!(load_fence_record(&info).await.unwrap(), None);
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(35));
    save_replication_bookmark(&info, 41).await.unwrap();
    assert_eq!(load_replication_bookmark(&info).await.unwrap(), 41);
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(35));
}

#[tokio::test]
async fn identity_update_preserves_horizon() {
    let info = store();
    save_bookmark_and_clean_horizon(&info, 50, 45)
        .await
        .unwrap();
    let fence = FenceRecord {
        bookmark: 52,
        lineage_id: [2; 16],
        history_epoch: 3,
        observed_identity: None,
    };
    save_fence_record(&info, fence).await.unwrap();
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(45));
}
