//! Regression test: `lookup_records_via_index` silently skips stale index
//! entries (index postings pointing at record IDs absent from the table).
//!
//! On the **pre-fix** code the per-record `self.get(id).await?` call
//! propagated `NotFound` as an error. The fixed code uses `get_many`
//! and skips `None` entries with `let … else { continue }`.
//!
//! The test plants a stale posting via the low-level
//! `IndexManager::on_record_created` API (called with a fabricated
//! `RecordId` never inserted into the table) and then drives an
//! index-backed `execute_delete` whose WHERE clause matches the
//! indexed field+value, forcing `lookup_records_via_index` to encounter
//! both the real and the stale id.

use crate::db_instance::db_instance::DbInstance;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_query_builder::{filter, write};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::{Repo, Store};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use tokio::sync::Notify;

/// Create an in-memory table, insert one real record with
/// `status = "active"`, create a regular index on `status`, plant a
/// stale posting (status="active" → fabricated RecordId), and return
/// the table manager, repo, and the real record's id.
async fn setup_table_with_stale_index_entry() -> (
    crate::table::TableManager,
    crate::repo::RepoInstance,
    RecordId,
) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();

    // --- Insert one real record with status="active" -----------------
    let interner = table.interner().get().await.unwrap();
    let mut map = new_map();
    map.insert("name".to_string(), QueryValue::Str("Alice".into()));
    map.insert("status".to_string(), QueryValue::Str("active".into()));
    let (inner_val, new_keys) =
        query_value_to_inner_tracked(&QueryValue::Map(map), interner).unwrap();
    if !new_keys.is_empty() {
        table.interner().save_new_keys(&new_keys).await.unwrap();
    }
    let real_id = table.insert(&inner_val).await.unwrap();

    // --- Create a regular index on "status" --------------------------
    table.create_index("status_idx", &["status"]).await.unwrap();

    // --- Plant a stale posting: status="active" → fake_id ------------
    // Build an InnerValue::Map with the interned "status" key so that
    // `on_record_created` sees the field and writes a posting.
    let interner = table.interner().get().await.unwrap();
    let mut inner_map = new_map();
    let status_key = interner
        .get_ind("status")
        .expect("'status' must be interned after insert + index creation");
    inner_map.insert(
        status_key,
        shamir_types::types::value::InnerValue::Str("active".into()),
    );
    let fake_value = shamir_types::types::value::InnerValue::Map(inner_map);

    let fake_id = RecordId::new();
    // This writes a posting (status="active" → fake_id) into the index
    // store, but fake_id has no corresponding record in the data store.
    table
        .index_manager_ref()
        .on_record_created(&fake_id, &fake_value)
        .await
        .unwrap();

    (table, repo, real_id)
}

/// Regression: `execute_delete` WHERE status="active" must succeed even
/// though the index contains a stale posting pointing at a non-existent
/// record.
///
/// Pre-fix behaviour: `lookup_records_via_index` called `self.get(id)`
/// per-record; the stale id caused a `NotFound` error → `Err(...)`.
///
/// Post-fix behaviour: `get_many` returns `None` for the stale id,
/// the `let Some(record) = record_opt else { continue }` guard skips
/// it, and only the real record is returned → `Ok(...)`.
#[tokio::test]
async fn test_stale_index_entry_skipped_in_lookup_records_via_index() {
    let (table, repo, _real_id) = setup_table_with_stale_index_entry().await;

    let refs = new_map();

    // Delete WHERE status = "active" — triggers index-backed path
    // because "status_idx" covers the Eq condition.
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .build();

    // This is the critical assertion: the operation must NOT error.
    // On the pre-fix code it would propagate NotFound for the stale id.
    let result = super::write_exec_tests::delete_via_tx(&repo, &table, &op, &refs)
        .await
        .unwrap();

    // Only the one real record (Alice) should have been deleted.
    assert_eq!(
        result.affected, 1,
        "exactly one real record should be deleted"
    );

    // Table should now be empty.
    assert_eq!(
        table.count().await.unwrap(),
        0,
        "table should be empty after deleting the only real record"
    );
}

// ===========================================================================
// S3.3 regression — the non-tx delete window exposes a stale covering posting
// ===========================================================================
//
// MVCC_CELL.md §4.2 proves the non-tx write path mutates the data store
// *before* it reconciles the sorted posting:
//
//     table.delete(id)            -> record gone from the data store
//     ...
//     sorted_indexes.on_record_deleted(id, old)  -> posting removed (later)
//
// Between those two steps a concurrent reader observes a sorted posting whose
// record no longer exists. The current full-fetch read path closes this with
// `get_many -> None -> skip`; an index-only read (S3.3) would serve the
// posting's covering projection directly and return a PHANTOM row, because it
// never fetches the record. This test pins that window deterministically.
//
// It is a *characterization* test (green today): it asserts the buggy window
// IS observable. When the atomic write-envelope (cell tact, S1.1) lands —
// applying `{data, postings, projection}` in one indivisible step — the
// window vanishes and the in-window assertions flip to "posting already gone".
//
// SORTED_TAG (the first byte of every sorted-index entry key) is 0x80; the
// pausable info-store below suspends precisely the posting-removal `remove`.

const SORTED_TAG: u8 = 0x80;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(Bytes, Bytes)>, DbError>> + Send>>;

/// An info-store wrapper that, when armed, suspends the first `remove()` of a
/// sorted-index posting (key prefixed with `SORTED_TAG`) until `release()` is
/// called — signalling `entered` the moment it parks. Every other operation
/// delegates straight through to the inner store.
struct PausableInfoStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
    entered: Arc<Notify>,
    gate: Arc<Notify>,
}

impl PausableInfoStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(Notify::new()),
            gate: Arc::new(Notify::new()),
        }
    }
    fn arm(&self) {
        self.armed.store(true, SeqCst);
    }
    fn release(&self) {
        self.gate.notify_one();
    }
}

#[async_trait]
impl Store for PausableInfoStore {
    async fn insert(&self, value: Bytes) -> DbResult<Bytes> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: Bytes, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: Bytes) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: Bytes) -> DbResult<bool> {
        // Park exactly once, only on the sorted-posting removal.
        if key.first() == Some(&SORTED_TAG) && self.armed.swap(false, SeqCst) {
            self.entered.notify_one();
            self.gate.notified().await;
        }
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size)
    }
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.inner
            .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size)
    }
}

/// Count sorted-index postings that still carry a non-empty covering
/// projection (`physical_value`). A non-empty value is exactly what an
/// index-only read would decode and return — for a record that may be gone.
async fn covering_postings_with_projection(info: &Arc<dyn Store>) -> usize {
    let stream = info.scan_prefix_stream(Bytes::from_static(&[SORTED_TAG]), 256);
    futures::pin_mut!(stream);
    let mut n = 0usize;
    while let Some(batch) = stream.next().await {
        for (_k, v) in batch.unwrap() {
            if !v.is_empty() {
                n += 1;
            }
        }
    }
    n
}

#[tokio::test]
async fn covering_delete_window_exposes_stale_posting() {
    // --- Build a TableManager over a pausable info-store -----------------
    let repo = Arc::new(InMemoryRepo::new());
    let data_store: Arc<dyn Store> = repo.store_get("__data__cov".to_string()).await.unwrap();
    let inner_info: Arc<dyn Store> = repo.store_get("__info__cov".to_string()).await.unwrap();
    let pausable = Arc::new(PausableInfoStore::new(Arc::clone(&inner_info)));
    let info_store: Arc<dyn Store> = Arc::clone(&pausable) as Arc<dyn Store>;

    let table = crate::table::TableManager::create("cov".to_string(), data_store, info_store)
        .await
        .unwrap();

    // Covering sorted index: sort on `score`, project `email`.
    table
        .create_sorted_index_with_include("score_idx", &["score"], vec![vec!["email".to_string()]])
        .await
        .unwrap();

    // Interned ids (create_sorted_index_with_include already interned them).
    let interner = table.interner().get().await.unwrap();
    let score_id = interner.touch_ind("score").unwrap().key().id();
    let email_id = interner.touch_ind("email").unwrap().key().id();
    let idx_name = interner.touch_ind("score_idx").unwrap().key().id();

    // Insert one record {score: 42, email: "alice@example.com"}.
    let mut m = new_map();
    m.insert(
        shamir_types::core::interner::InternerKey::new(score_id),
        shamir_types::types::value::InnerValue::Int(42),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(email_id),
        shamir_types::types::value::InnerValue::Str("alice@example.com".into()),
    );
    let rec = shamir_types::types::value::InnerValue::Map(m);
    let id = table.insert(&rec).await.unwrap();

    // Sanity: one posting, with a covering projection.
    assert_eq!(
        covering_postings_with_projection(&inner_info).await,
        1,
        "insert must produce one covering posting with a projection"
    );

    // --- Arm, then run the real delete on a separate task ---------------
    pausable.arm();
    let table_bg = table.clone();
    let handle = tokio::spawn(async move { table_bg.delete(id).await });

    // Wait until the delete has removed the record but parked right before
    // reconciling the sorted posting.
    pausable.entered.notified().await;

    // --- Observe the window ---------------------------------------------
    // The record is already gone…
    assert_eq!(
        table.count().await.unwrap(),
        0,
        "record is already removed from the data store inside the window"
    );
    // …but the sorted posting is still present…
    let ids = table
        .sorted_indexes()
        .lookup_range(idx_name, None, None)
        .await
        .unwrap();
    assert!(
        ids.contains(&id),
        "stale covering posting still points at the deleted record"
    );
    // …and it still carries the covering projection — an index-only read
    // would decode this and return a PHANTOM row. THIS is the bug S3.3
    // would expose; the atomic write-envelope will close the window.
    assert_eq!(
        covering_postings_with_projection(&inner_info).await,
        1,
        "the stale posting still serves a covering projection in the window"
    );

    // --- Release and let the delete finish ------------------------------
    pausable.release();
    handle.await.unwrap().unwrap();

    // After the delete completes the posting is gone — the full-fetch path
    // never had to observe it because it routes through get_many.
    let ids_after = table
        .sorted_indexes()
        .lookup_range(idx_name, None, None)
        .await
        .unwrap();
    assert!(
        ids_after.is_empty(),
        "posting must be removed once the delete completes"
    );
    assert_eq!(
        covering_postings_with_projection(&inner_info).await,
        0,
        "no covering projection remains after the delete completes"
    );
}
