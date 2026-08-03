//! F-78 (#905) correctness-equivalence test.
//!
//! The streaming rewrite ([`IndexManager::create_index_from_stream`]) must
//! produce the SAME set of postings as the old materializing path
//! ([`IndexManager::create_index_from_records`]) for an identical fixture —
//! only HOW the postings are written changes (stream-and-batch vs.
//! materialize-then-one-`set_many`), never WHAT.
//!
//! This builds a fixture exercising:
//! - distinct indexed values (one posting each),
//! - low-cardinality collisions (many records → same indexed value → many
//!   postings under one 25-byte index key, distinguished by record_id), and
//! - records MISSING the indexed field (no posting —
//!   `build_index_key_from_record` returns `None`),
//!
//! then runs BOTH paths against byte-identical stores and asserts the
//! resulting posting SETS are byte-identical. The streaming path additionally
//! consumes the fixture in odd-sized (non-record-count-aligned) batches to
//! exercise the per-batch `set_many` arm.

use std::sync::Arc;

use bytes::Bytes;
use futures::stream;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_manager::IndexManager;
use crate::base_index::index_record_key::IndexRecordKey;

const FIELD_ID: u64 = 7777;
const NAME_INTERNED: u64 = 9050;

/// Build a fixture of `n` records exercising distinct values, low-cardinality
/// collisions, and field-absent rows (which produce no posting).
fn build_fixture(n: usize) -> Vec<(RecordId, InnerValue)> {
    let mut out = Vec::with_capacity(n);
    let base_ts = RecordId::now_micros();
    for i in 0..n {
        let rid = RecordId::from_ts_seq(base_ts, i as u32);
        let mut map = new_map();
        match i % 5 {
            // Field absent → no posting (`build_index_key_from_record` → None).
            0 => {}
            // Two collision buckets sharing the value "dup_a" (~40% of rows).
            1 | 2 => {
                map.insert(InternerKey::new(FIELD_ID), InnerValue::Str("dup_a".into()));
            }
            // A third collision bucket over a small integer domain (i % 3).
            3 => {
                map.insert(InternerKey::new(FIELD_ID), InnerValue::Int((i % 3) as i64));
            }
            // A distinct string value per row.
            _ => {
                map.insert(
                    InternerKey::new(FIELD_ID),
                    InnerValue::Str(format!("v_{i}")),
                );
            }
        }
        out.push((rid, InnerValue::Map(map)));
    }
    out
}

/// Scan an `info_store` for every posting of the given index and collect the
/// `(key, value)` pairs into a sorted set. Postings live under the index's
/// 9-byte prefix (`is_unique || name_interned`); the `system:indexes`
/// metadata key lives in a disjoint namespace and is not returned.
async fn collect_postings(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
) -> std::collections::BTreeSet<(Vec<u8>, Vec<u8>)> {
    let prefix: Bytes = IndexRecordKey::new(false, name_interned).to_prefix_bytes();
    let mut out = std::collections::BTreeSet::new();
    let mut s = info_store.scan_prefix_stream(prefix, 1000);
    while let Some(batch) = s.next().await {
        for (k, v) in batch.unwrap() {
            out.insert((k.as_ref().to_vec(), v.as_ref().to_vec()));
        }
    }
    out
}

/// OLD (materialize-then-one-`set_many`) vs NEW (stream-and-batch) builds of
/// the SAME index over an identical fixture must yield byte-identical posting
/// sets.
#[tokio::test]
async fn streaming_matches_materializing_posting_sets() {
    let n = 5_000;
    let fixture = build_fixture(n);
    let paths = vec![IndexInfoItem::new(vec![FIELD_ID])];

    // ── OLD: materializing path ───────────────────────────────────────────
    let data_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_old = IndexManager::new(Arc::clone(&data_old), Arc::clone(&info_old))
        .await
        .unwrap();
    let def_old = IndexDefinition::new(NAME_INTERNED, paths.clone());
    mgr_old
        .create_index_from_records(def_old, fixture.clone())
        .await
        .unwrap();
    let postings_old = collect_postings(&info_old, NAME_INTERNED).await;

    // ── NEW: streaming path ───────────────────────────────────────────────
    // Chunk the SAME fixture into odd-sized (137-row) batches to exercise the
    // multi-batch `set_many` arm — 5000 / 137 ≈ 37 batches, none aligned to a
    // whole-table boundary.
    let data_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_new = IndexManager::new(Arc::clone(&data_new), Arc::clone(&info_new))
        .await
        .unwrap();
    let def_new = IndexDefinition::new(NAME_INTERNED, paths.clone());
    let chunks: Vec<_> = fixture.chunks(137).map(|c| Ok(c.to_vec())).collect();
    let stream = stream::iter(chunks);
    mgr_new
        .create_index_from_stream(def_new, stream)
        .await
        .unwrap();
    let postings_new = collect_postings(&info_new, NAME_INTERNED).await;

    assert_eq!(
        postings_old, postings_new,
        "F-78: streaming and materializing builds must produce byte-identical \
         posting sets"
    );
    // Sanity: the fixture actually produced a substantial, non-trivial posting
    // set (4/5 of rows index → ~4000 postings for n=5000). Guards against a
    // both-sides-empty false-pass.
    assert!(
        postings_old.len() > n / 2,
        "expected a substantial posting set, got {}",
        postings_old.len()
    );
}

/// A one-record-per-batch stream (batch size 1) must still match the
/// materializing path — exercises the degenerate batch boundary and confirms
/// many tiny `set_many` calls accumulate identically to one big one.
#[tokio::test]
async fn streaming_single_record_batches_match_materializing() {
    let n = 500;
    let fixture = build_fixture(n);
    let paths = vec![IndexInfoItem::new(vec![FIELD_ID])];

    let data_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_old = IndexManager::new(Arc::clone(&data_old), Arc::clone(&info_old))
        .await
        .unwrap();
    let def_old = IndexDefinition::new(NAME_INTERNED + 1, paths.clone());
    mgr_old
        .create_index_from_records(def_old, fixture.clone())
        .await
        .unwrap();
    let postings_old = collect_postings(&info_old, NAME_INTERNED + 1).await;

    let data_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_new = IndexManager::new(Arc::clone(&data_new), Arc::clone(&info_new))
        .await
        .unwrap();
    let def_new = IndexDefinition::new(NAME_INTERNED + 1, paths.clone());
    // One record per batch.
    let chunks: Vec<_> = fixture.iter().map(|r| Ok(vec![r.clone()])).collect();
    let stream = stream::iter(chunks);
    mgr_new
        .create_index_from_stream(def_new, stream)
        .await
        .unwrap();
    let postings_new = collect_postings(&info_new, NAME_INTERNED + 1).await;

    assert_eq!(
        postings_old, postings_new,
        "F-78: single-record-batch streaming must match materializing build"
    );
}

/// An empty table must produce an empty (but error-free) index build via the
/// streaming path, matching the materializing path.
#[tokio::test]
async fn streaming_empty_table_matches_materializing() {
    let paths = vec![IndexInfoItem::new(vec![FIELD_ID])];

    let data_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_old: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_old = IndexManager::new(Arc::clone(&data_old), Arc::clone(&info_old))
        .await
        .unwrap();
    let def_old = IndexDefinition::new(NAME_INTERNED + 2, paths.clone());
    mgr_old
        .create_index_from_records(def_old, Vec::new())
        .await
        .unwrap();
    let postings_old = collect_postings(&info_old, NAME_INTERNED + 2).await;

    let data_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_new: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr_new = IndexManager::new(Arc::clone(&data_new), Arc::clone(&info_new))
        .await
        .unwrap();
    let def_new = IndexDefinition::new(NAME_INTERNED + 2, paths.clone());
    let stream = stream::iter(Vec::new());
    mgr_new
        .create_index_from_stream(def_new, stream)
        .await
        .unwrap();
    let postings_new = collect_postings(&info_new, NAME_INTERNED + 2).await;

    assert!(postings_old.is_empty());
    assert_eq!(postings_old, postings_new);
}
