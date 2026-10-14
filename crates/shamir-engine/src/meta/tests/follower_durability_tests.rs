use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};

use crate::meta::recovery_marker::save_u64;
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark,
    save_bookmark_and_clean_horizon, save_fence_record, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaKey};

fn info() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

#[tokio::test]
async fn legacy_u64_and_v1_bookmarks_decode_with_unknown_horizon() {
    let store = info();
    save_u64(&store, MetaKey::ReplicationBookmark, 23)
        .await
        .unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 23);
    assert_eq!(load_clean_horizon(&store).await.unwrap(), None);

    let record = FenceRecord {
        bookmark: 24,
        lineage_id: [3; 16],
        history_epoch: 2,
        observed_identity: None,
    };
    save_fence_record(&store, record).await.unwrap();
    assert_eq!(load_fence_record(&store).await.unwrap(), Some(record));
}

#[tokio::test]
async fn versioned_record_keeps_upstream_bookmark_and_local_j_distinct() {
    let store = info();
    let bytes = MetaEnvelope::new((2u16, (None::<FenceRecord>, 900u64, Some(12u64))))
        .encode()
        .unwrap();
    store
        .set(
            RecordKey::from_slice(
                crate::meta::namespace::MetaKey::ReplicationBookmark
                    .as_record_id()
                    .as_bytes(),
            ),
            Bytes::from(bytes),
        )
        .await
        .unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 900);
    assert_eq!(load_clean_horizon(&store).await.unwrap(), Some(12));
    save_bookmark_and_clean_horizon(&store, 901, 15)
        .await
        .unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 901);
}
