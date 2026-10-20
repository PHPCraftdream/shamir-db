use crate::meta::repl::{
    decode_bookmark_v3, encode_bookmark_v3, load_bookmark_v3, save_bookmark_v3, BookmarkV3,
};
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark, FenceRecord,
};
use crate::meta::{MetaEnvelope, MetaKey};
use bytes::Bytes;
use shamir_storage::error::{DbError, FollowerRefusalReason};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use std::sync::Arc;

fn store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn bookmark_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationBookmark.as_record_id().as_bytes())
}

fn fence(bookmark: u64) -> FenceRecord {
    FenceRecord {
        bookmark,
        lineage_id: [5; 16],
        history_epoch: 3,
        observed_identity: None,
    }
}

#[tokio::test]
async fn v3_record_round_trips() {
    let info = store();
    let record = BookmarkV3 {
        fence: Some(fence(30)),
        bookmark: 30,
        clean_through: Some(28),
        outstanding_receipt: Some([7; 16]),
    };
    let bytes = encode_bookmark_v3(&record).unwrap();
    info.set(bookmark_key(), Bytes::from(bytes.clone()))
        .await
        .unwrap();
    assert_eq!(load_bookmark_v3(&info).await.unwrap().unwrap(), record);
    assert_eq!(decode_bookmark_v3(&bytes).unwrap(), record);
}

#[tokio::test]
async fn absent_key_loads_none() {
    let info = store();
    assert!(load_bookmark_v3(&info).await.unwrap().is_none());
}

/// The strict loaders are projections of a valid v3 record.
#[tokio::test]
async fn projections_read_v3_records() {
    let info = store();
    save_bookmark_v3(
        &info,
        &BookmarkV3 {
            fence: Some(fence(30)),
            bookmark: 30,
            clean_through: Some(28),
            outstanding_receipt: Some([7; 16]),
        },
    )
    .await
    .unwrap();
    assert_eq!(load_replication_bookmark(&info).await.unwrap(), 30);
    assert_eq!(load_fence_record(&info).await.unwrap(), Some(fence(30)));
    assert_eq!(load_clean_horizon(&info).await.unwrap(), Some(28));
}

#[tokio::test]
async fn unknown_version_fails_closed() {
    let info = store();
    let bytes = MetaEnvelope::new((
        9u16,
        BookmarkV3 {
            fence: None,
            bookmark: 5,
            clean_through: None,
            outstanding_receipt: None,
        },
    ))
    .encode()
    .unwrap();
    info.set(bookmark_key(), Bytes::from(bytes.clone()))
        .await
        .unwrap();
    assert!(matches!(
        load_bookmark_v3(&info).await,
        Err(DbError::FollowerStateRefused {
            reason: FollowerRefusalReason::MalformedBookmark
        })
    ));
    assert!(decode_bookmark_v3(&bytes).is_err());
}

#[tokio::test]
async fn corrupt_bytes_fail_closed() {
    let info = store();
    let record = BookmarkV3 {
        fence: Some(fence(30)),
        bookmark: 30,
        clean_through: Some(28),
        outstanding_receipt: Some([7; 16]),
    };
    let bytes = encode_bookmark_v3(&record).unwrap();

    let mut truncated = bytes.clone();
    truncated.truncate(bytes.len() / 2);
    info.set(bookmark_key(), Bytes::from(truncated.clone()))
        .await
        .unwrap();
    assert!(load_bookmark_v3(&info).await.is_err());
    assert!(decode_bookmark_v3(&truncated).is_err());

    let mut garbage = vec![0u8; 64];
    for (i, byte) in garbage.iter_mut().enumerate() {
        *byte = (i * 91 % 253) as u8;
    }
    info.set(bookmark_key(), Bytes::from(garbage.clone()))
        .await
        .unwrap();
    assert!(load_bookmark_v3(&info).await.is_err());
    assert!(decode_bookmark_v3(&garbage).is_err());
}

#[tokio::test]
async fn v3_consistency_check_rejects_mismatched_fence() {
    let mismatched = BookmarkV3 {
        fence: Some(fence(99)),
        bookmark: 30,
        clean_through: None,
        outstanding_receipt: None,
    };
    // Hand-build the envelope so the encoder's own record is bypassed.
    let bytes = MetaEnvelope::new((3u16, mismatched)).encode().unwrap();
    assert!(matches!(
        decode_bookmark_v3(&bytes),
        Err(DbError::Storage(_))
    ));
}

#[tokio::test]
async fn v3_rejects_trailing_bytes() {
    let record = BookmarkV3 {
        fence: None,
        bookmark: 5,
        clean_through: None,
        outstanding_receipt: None,
    };
    let mut bytes = encode_bookmark_v3(&record).unwrap();
    bytes.push(0);
    assert!(decode_bookmark_v3(&bytes).is_err());
}
