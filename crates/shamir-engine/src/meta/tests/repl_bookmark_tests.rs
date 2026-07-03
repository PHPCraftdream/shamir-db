//! R1-b — codec-level tests for the durable replication bookmark
//! (`meta::repl_bookmark`).
//!
//! The repo-level contract (default-0, monotonic advance, reopen survival)
//! is exercised in `crate::repo::tests::repl_bookmark_tests`. These tests
//! cover the bare store helpers: default-0 on a fresh store, round-trip,
//! and that the bookmark key does NOT collide with `LastCommittedVersion`
//! or `NextTxId` (each marker lives under its own `_t.*` tag).

use crate::meta::recovery_marker::{
    load_last_committed, load_next_tx_id_snapshot, save_last_committed, save_next_tx_id_snapshot,
};
use crate::meta::repl_bookmark::{load_replication_bookmark, save_replication_bookmark};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use std::sync::Arc;

fn mem_store() -> Arc<dyn Store> {
    Arc::new(InMemoryStore::new())
}

/// Fresh store → `load_replication_bookmark` returns 0 (the default; no
/// marker has ever been written).
#[tokio::test]
async fn load_missing_returns_zero() {
    let s = mem_store();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 0);
}

/// `save` then `load` round-trips the value verbatim.
#[tokio::test]
async fn bookmark_round_trip() {
    let s = mem_store();
    save_replication_bookmark(&s, 1_337).await.unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 1_337);
}

/// A second `save` overwrites the first (the store helper is a plain
/// high-water setter; monotonicity is enforced at the repo-level wrapper).
#[tokio::test]
async fn bookmark_overwrites() {
    let s = mem_store();
    save_replication_bookmark(&s, 1).await.unwrap();
    save_replication_bookmark(&s, 999).await.unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 999);
}

/// The bookmark key (`_t.rbm`) is distinct from `LastCommittedVersion`
/// (`_t.lcv`) and `NextTxId` (`_t.nti`): writing all three must leave each
/// readable at its own value, proving no tag collision.
#[tokio::test]
async fn bookmark_does_not_collide_with_other_markers() {
    let s = mem_store();
    save_replication_bookmark(&s, 10).await.unwrap();
    save_last_committed(&s, 20).await.unwrap();
    save_next_tx_id_snapshot(&s, 30).await.unwrap();
    assert_eq!(load_replication_bookmark(&s).await.unwrap(), 10);
    assert_eq!(load_last_committed(&s).await.unwrap(), Some(20));
    assert_eq!(load_next_tx_id_snapshot(&s).await.unwrap(), Some(30));
}
