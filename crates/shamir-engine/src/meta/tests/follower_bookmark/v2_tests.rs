use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};

use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark,
    save_bookmark_and_clean_horizon, save_fence_record, save_fence_record_v3_aware,
    save_replication_bookmark, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaKey};

fn store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn bookmark_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

#[tokio::test]
async fn fresh_v3_aware_identity_fence_uses_absent_bookmark_cursor() {
    let info = store();
    let mut adopted = FenceRecord {
        bookmark: 0,
        lineage_id: [8; 16],
        history_epoch: 9,
        observed_identity: None,
    };
    // No bookmark means cursor zero is authoritative even if a caller passes
    // a stale/nonzero cursor; v3 requires fence.bookmark == flat bookmark.
    adopted.bookmark = 17;
    save_fence_record_v3_aware(&info, adopted).await.unwrap();
    let v3 = crate::meta::repl::load_bookmark_v3(&info)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v3.bookmark, 0);
    assert_eq!(v3.fence.unwrap().bookmark, 0);
}

#[tokio::test]
async fn legacy_marker_without_bookmark_keeps_identity_adoption_legacy() {
    let info = store();
    crate::meta::recovery_marker::save_last_committed(&info, 7)
        .await
        .unwrap();
    let fence = FenceRecord {
        bookmark: 0,
        lineage_id: [8; 16],
        history_epoch: 9,
        observed_identity: None,
    };
    save_fence_record_v3_aware(&info, fence).await.unwrap();
    let bytes = info.get(bookmark_key()).await.unwrap();
    let (version, decoded): (u16, FenceRecord) = MetaEnvelope::open(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(decoded, fence);
}

#[tokio::test]
async fn fence_save_without_horizon_writes_v1_wire_record() {
    let info = store();
    let fence = FenceRecord {
        bookmark: 0,
        lineage_id: [4; 16],
        history_epoch: u64::MAX,
        observed_identity: None,
    };
    save_fence_record(&info, fence).await.unwrap();
    let bytes = info.get(bookmark_key()).await.unwrap();
    assert_eq!(bytes.len(), 49);
    let (version, decoded): (u16, FenceRecord) = MetaEnvelope::open(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(decoded, fence);
}

#[tokio::test]
async fn authoritative_fence_save_replaces_invalid_previous_record() {
    let info = store();
    let fence = FenceRecord {
        bookmark: u64::MAX,
        lineage_id: [9; 16],
        history_epoch: 0,
        observed_identity: None,
    };
    let mut corrupt = MetaEnvelope::new((1u16, fence)).encode().unwrap();
    corrupt.push(0);
    info.set(bookmark_key(), Bytes::from(corrupt))
        .await
        .unwrap();
    save_fence_record(&info, fence).await.unwrap();
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence));
}

#[tokio::test]
async fn v1_and_v2_records_reject_trailing_bytes() {
    let info = store();
    let fence = FenceRecord {
        bookmark: 8,
        lineage_id: [3; 16],
        history_epoch: 7,
        observed_identity: None,
    };
    for bytes in [
        MetaEnvelope::new((1u16, fence)).encode().unwrap(),
        MetaEnvelope::new((2u16, (None::<FenceRecord>, 8u64, Some(6u64))))
            .encode()
            .unwrap(),
        MetaEnvelope::new((2u16, (Some(fence), fence.bookmark, None::<u64>)))
            .encode()
            .unwrap(),
        MetaEnvelope::new((2u16, (Some(fence), fence.bookmark, Some(6u64))))
            .encode()
            .unwrap(),
    ] {
        let mut trailing = bytes;
        trailing.push(0);
        info.set(bookmark_key(), Bytes::from(trailing))
            .await
            .unwrap();
        assert!(load_replication_bookmark(&info).await.is_err());
        assert!(load_fence_record(&info).await.is_err());
        assert!(load_clean_horizon(&info).await.is_err());
    }
    info.set(
        bookmark_key(),
        Bytes::from(
            MetaEnvelope::new((2u16, (None::<FenceRecord>, 8u64, None::<u64>)))
                .encode()
                .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(load_clean_horizon(&info).await.unwrap(), None);
}

#[tokio::test]
async fn v2_horizon_preserves_fence_epoch_and_observed_identity_at_boundaries() {
    let info = store();
    let fence = FenceRecord {
        bookmark: 0,
        lineage_id: [5; 16],
        history_epoch: u64::MAX,
        observed_identity: Some(crate::meta::repo_identity::RepoIdentity {
            lineage_id: [6; 16],
            history_epoch: u64::MAX,
        }),
    };
    save_fence_record(&info, fence).await.unwrap();
    save_bookmark_and_clean_horizon(&info, u64::MAX, u64::MAX)
        .await
        .unwrap();
    let mut advanced = fence;
    advanced.bookmark = u64::MAX;
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(advanced));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(u64::MAX));
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

    let without_horizon = store();
    save_replication_bookmark(&without_horizon, 44)
        .await
        .unwrap();
    assert_eq!(
        load_replication_bookmark(&without_horizon).await.unwrap(),
        44
    );
    assert_eq!(load_fence_record(&without_horizon).await.unwrap(), None);
    assert_eq!(load_clean_horizon(&without_horizon).await.unwrap(), None);
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
