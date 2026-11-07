//! Strict-refusal tests (S5c-X PR6b-3): legacy u64/v1/v2 bookmark records
//! (including bookmark zero), truncated and unknown-version records are
//! refused with typed errors; corrupt records are never overwritten as
//! "fresh"; the floor-without-bookmark and fresh classifications behave.

use crate::meta::recovery_marker::{load_last_committed, save_last_committed};
use crate::meta::repl::{
    classify_follower_bookmark, load_bookmark_v3, save_bookmark_v3, BookmarkV3,
    FollowerBookmarkState,
};
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark, save_fence_record_v3_aware,
    FenceRecord,
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

/// The tests own the legacy fixture bytes — no production legacy writer
/// exists anymore.
fn legacy_fixtures() -> Vec<(Vec<u8>, FollowerRefusalReason)> {
    vec![
        // Fixed-width u64 bookmark records, including bookmark zero.
        (
            MetaEnvelope::new(23u64).encode().unwrap(),
            FollowerRefusalReason::LegacyBookmarkFormat,
        ),
        (
            MetaEnvelope::new(0u64).encode().unwrap(),
            FollowerRefusalReason::LegacyBookmarkFormat,
        ),
        // v1: bookmark bound to its upstream fence.
        (
            MetaEnvelope::new((1u16, fence(10))).encode().unwrap(),
            FollowerRefusalReason::LegacyBookmarkFormat,
        ),
        // v2: fence + independent clean horizon, including bookmark zero.
        (
            MetaEnvelope::new((2u16, (Some(fence(10)), 10u64, Some(8u64))))
                .encode()
                .unwrap(),
            FollowerRefusalReason::LegacyBookmarkFormat,
        ),
        (
            MetaEnvelope::new((2u16, (None::<FenceRecord>, 0u64, None::<u64>)))
                .encode()
                .unwrap(),
            FollowerRefusalReason::LegacyBookmarkFormat,
        ),
    ]
}

async fn assert_refused(info: &Arc<dyn Store>, reason: FollowerRefusalReason) {
    let check = |result: DbError| {
        assert!(
            matches!(result, DbError::FollowerStateRefused { reason: r } if r == reason),
            "expected FollowerStateRefused({reason:?}), got {result:?}"
        );
    };
    check(load_bookmark_v3(info).await.unwrap_err());
    check(classify_follower_bookmark(info).await.unwrap_err());
    check(load_replication_bookmark(info).await.unwrap_err());
    check(load_fence_record(info).await.unwrap_err());
    check(load_clean_horizon(info).await.unwrap_err());
}

#[tokio::test]
async fn legacy_records_refuse_every_loader_with_legacy_format() {
    for (bytes, reason) in legacy_fixtures() {
        let info = store();
        info.set(bookmark_key(), Bytes::from(bytes.clone()))
            .await
            .unwrap();
        assert_refused(&info, reason).await;
    }
}

#[tokio::test]
async fn truncated_and_unknown_version_records_refuse_as_malformed() {
    // Truncated below the envelope header: no readable version at all.
    let info = store();
    let mut truncated = MetaEnvelope::new((1u16, fence(10))).encode().unwrap();
    truncated.truncate(10);
    info.set(bookmark_key(), Bytes::from(truncated))
        .await
        .unwrap();
    assert_refused(&info, FollowerRefusalReason::MalformedBookmark).await;

    // An unknown tuple version is malformed, not legacy.
    let info = store();
    let unknown = MetaEnvelope::new((9u16, fence(10))).encode().unwrap();
    info.set(bookmark_key(), Bytes::from(unknown))
        .await
        .unwrap();
    assert_refused(&info, FollowerRefusalReason::MalformedBookmark).await;
}

#[tokio::test]
async fn truncated_v3_record_refuses_as_malformed() {
    // Bytes that SNIFF as v3 (header intact) but fail the strict decode are
    // a typed corruption refusal, not a generic internal error.
    let info = store();
    let mut truncated = MetaEnvelope::new((3u16, fence(10))).encode().unwrap();
    assert!(truncated.len() > 16);
    truncated.truncate(truncated.len() - 2);
    info.set(bookmark_key(), Bytes::from(truncated))
        .await
        .unwrap();
    assert_refused(&info, FollowerRefusalReason::MalformedBookmark).await;
}

#[tokio::test]
async fn adoption_never_overwrites_a_refused_record() {
    for (bytes, _) in legacy_fixtures() {
        let info = store();
        info.set(bookmark_key(), Bytes::from(bytes.clone()))
            .await
            .unwrap();
        assert!(save_fence_record_v3_aware(&info, fence(99)).await.is_err());
        // The stored bytes are untouched — no rewrite, no v3 mint.
        assert_eq!(info.get(bookmark_key()).await.unwrap(), Bytes::from(bytes));
    }
}

#[tokio::test]
async fn floor_without_bookmark_classifies_and_refuses_adoption() {
    let info = store();
    save_last_committed(&info, 7).await.unwrap();
    assert!(matches!(
        classify_follower_bookmark(&info).await.unwrap(),
        FollowerBookmarkState::FloorWithoutBookmark { floor: 7 }
    ));
    assert!(matches!(
        save_fence_record_v3_aware(&info, fence(0)).await,
        Err(DbError::FollowerStateRefused {
            reason: FollowerRefusalReason::LegacyFollowerState
        })
    ));
    // Nothing was written over the floor.
    assert!(load_bookmark_v3(&info).await.unwrap().is_none());
    assert_eq!(load_last_committed(&info).await.unwrap(), Some(7));
}

#[tokio::test]
async fn fresh_store_classifies_fresh_and_adopts_v3_baseline() {
    let info = store();
    assert!(matches!(
        classify_follower_bookmark(&info).await.unwrap(),
        FollowerBookmarkState::Fresh
    ));
    // A stale/nonzero cursor in the adopted fence is overridden: bookmark
    // zero is authoritative before any apply.
    save_fence_record_v3_aware(&info, fence(17)).await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(
        v3,
        BookmarkV3 {
            fence: Some(fence(0)),
            bookmark: 0,
            clean_through: None,
            outstanding_receipt: None,
        }
    );
    // After the baseline exists the store classifies as a native follower.
    assert!(matches!(
        classify_follower_bookmark(&info).await.unwrap(),
        FollowerBookmarkState::Native(_)
    ));
}

#[tokio::test]
async fn adoption_preserves_bookmark_clean_horizon_and_receipt() {
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
    save_fence_record_v3_aware(&info, fence(31)).await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.bookmark, 30);
    assert_eq!(v3.clean_through, Some(28));
    assert_eq!(v3.outstanding_receipt, Some([7; 16]));
    assert_eq!(v3.fence, Some(fence(30)));
    // A fence written with the record's own bookmark is kept verbatim.
    save_fence_record_v3_aware(&info, fence(30)).await.unwrap();
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.fence, Some(fence(30)));
}
