//! R1-b — codec-level tests for the durable replication bookmark
//! (`meta::repl_bookmark`, strict v3-only contract).
//!
//! The repo-level contract (default-0, monotonic advance, reopen survival)
//! is exercised in `crate::repo::tests::repl_bookmark_tests`. These tests
//! cover the bare store helpers: default-0 on a fresh store, round-trip
//! through the v3 record, and that the bookmark key does NOT collide with
//! `LastCommittedVersion` or `NextTxId` (each marker lives under its own
//! `_t.*` tag). Refusal fixtures live in
//! `crate::meta::repl::tests::bookmark_refusal_tests`.

use crate::meta::recovery_marker::{
    load_last_committed, load_next_tx_id_snapshot, save_last_committed, save_next_tx_id_snapshot,
};
use crate::meta::repl::{save_bookmark_v3, BookmarkV3};
use crate::meta::repl_bookmark::{
    load_clean_horizon, load_fence_record, load_replication_bookmark, FenceRecord,
};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

fn fence(bookmark: u64) -> FenceRecord {
    FenceRecord {
        bookmark,
        lineage_id: [5; 16],
        history_epoch: 3,
        observed_identity: None,
    }
}

/// Fresh store → `load_replication_bookmark` returns 0 (the default; no
/// marker has ever been written).
#[tokio::test]
async fn load_missing_returns_zero() {
    let s = mem_store();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 0);
    assert_eq!(load_fence_record(&s).await.unwrap(), None);
    assert_eq!(load_clean_horizon(&s).await.unwrap(), None);
}

/// A v3 record round-trips through the projections verbatim.
#[tokio::test]
async fn bookmark_round_trip() {
    let s = mem_store();
    save_bookmark_v3(
        &s,
        &BookmarkV3 {
            fence: Some(fence(1_337)),
            bookmark: 1_337,
            clean_through: Some(1_300),
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 1_337);
    assert_eq!(load_fence_record(&s).await.unwrap(), Some(fence(1_337)));
    assert_eq!(load_clean_horizon(&s).await.unwrap(), Some(1_300));
}

/// Writing a new v3 record overwrites the first (the store helper is a plain
/// high-water setter; monotonicity is enforced at the repo-level wrapper).
#[tokio::test]
async fn bookmark_overwrites() {
    let s = mem_store();
    save_bookmark_v3(
        &s,
        &BookmarkV3 {
            fence: None,
            bookmark: 1,
            clean_through: None,
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    save_bookmark_v3(
        &s,
        &BookmarkV3 {
            fence: None,
            bookmark: 999,
            clean_through: None,
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 999);
}

/// The bookmark key (`_t.rbm`) is distinct from `LastCommittedVersion`
/// (`_t.lcv`) and `NextTxId` (`_t.nti`): writing all three must leave each
/// readable at its own value, proving no tag collision.
#[tokio::test]
async fn bookmark_does_not_collide_with_other_markers() {
    let s = mem_store();
    save_bookmark_v3(
        &s,
        &BookmarkV3 {
            fence: None,
            bookmark: 10,
            clean_through: None,
            outstanding_receipt: None,
        },
    )
    .await
    .unwrap();
    save_last_committed(&s, 20).await.unwrap();
    save_next_tx_id_snapshot(&s, 30).await.unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 10);
    assert_eq!(load_last_committed(&s).await.unwrap(), Some(20));
    assert_eq!(load_next_tx_id_snapshot(&s).await.unwrap(), Some(30));
}
