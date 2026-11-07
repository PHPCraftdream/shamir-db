//! Durable-follower fence/bookmark metadata tests (strict v3-only).
//!
//! Identity adoption (`save_fence_record_v3_aware`) must preserve the v3
//! record's bookmark, clean horizon and outstanding-receipt pointer, and the
//! projections must read them back distinctly. Refusal fixtures live in
//! `crate::meta::repl::tests::bookmark_refusal_tests`.

use crate::meta::repl::{load_bookmark_v3, save_bookmark_v3, BookmarkV3};
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark, save_fence_record_v3_aware,
    FenceRecord,
};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use std::sync::Arc;

fn info() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn fence(bookmark: u64) -> FenceRecord {
    FenceRecord {
        bookmark,
        lineage_id: [3; 16],
        history_epoch: 2,
        observed_identity: None,
    }
}

/// Fresh adoption writes a v3 baseline whose cursor is zero, regardless of a
/// stale cursor in the adopted fence (v3 requires fence.bookmark == bookmark).
#[tokio::test]
async fn fresh_v3_aware_identity_fence_uses_absent_bookmark_cursor() {
    let store = info();
    save_fence_record_v3_aware(&store, fence(17)).await.unwrap();
    let v3 = load_bookmark_v3(&store).await.unwrap().unwrap();
    assert_eq!(v3.bookmark, 0);
    assert_eq!(v3.fence, Some(fence(0)));
    assert_eq!(v3.clean_through, None);
    assert_eq!(v3.outstanding_receipt, None);
}

/// A fence write over an established v3 record preserves the record's
/// bookmark, clean horizon and receipt pointer exactly.
#[tokio::test]
async fn fence_write_preserves_bookmark_horizon_and_receipt() {
    let store = info();
    save_bookmark_v3(
        &store,
        &BookmarkV3 {
            fence: Some(fence(900)),
            bookmark: 900,
            clean_through: Some(12),
            outstanding_receipt: Some([9; 16]),
        },
    )
    .await
    .unwrap();
    save_fence_record_v3_aware(&store, fence(901))
        .await
        .unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 900);
    assert_eq!(load_clean_horizon(&store).await.unwrap(), Some(12));
    let v3 = load_bookmark_v3(&store).await.unwrap().unwrap();
    assert_eq!(v3.fence, Some(fence(900)));
    assert_eq!(v3.outstanding_receipt, Some([9; 16]));
}

/// Upstream bookmark and follower-local clean horizon (J) stay distinct
/// through a fence write.
#[tokio::test]
async fn versioned_record_keeps_upstream_bookmark_and_local_j_distinct() {
    let store = info();
    save_bookmark_v3(
        &store,
        &BookmarkV3 {
            fence: None,
            bookmark: 900,
            clean_through: Some(12),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 900);
    assert_eq!(load_clean_horizon(&store).await.unwrap(), Some(12));
    save_fence_record_v3_aware(&store, fence(0)).await.unwrap();
    assert_eq!(load_replication_bookmark(&store).await.unwrap(), 900);
    assert_eq!(load_fence_record(&store).await.unwrap(), Some(fence(900)));
    assert_eq!(load_clean_horizon(&store).await.unwrap(), Some(12));
}
