//! F-30 (#823) — `QueryResult::corrupt_records` regression tests for the
//! BYTE-LEVEL twins `try_project_page_only_bytes` / `apply_select_value_bytes`
//! (`read_exec.rs`) and their call sites in `read_index_scan.rs` /
//! `read_temporal.rs`.
//!
//! F-10 (#800, `corrupt_record_tests.rs`) covered `read_exec.rs`'s OWN scan
//! loops. This file covers the sibling gap it explicitly deferred: the two
//! free functions have no `QueryResult` directly in scope, so a corrupt row
//! silently vanished from `records` with no `corrupt_records` entry — the
//! ROW was still (correctly) dropped, but the drop was unreported. F-30
//! threads a `&mut Vec<CorruptRecordRef>` accumulator through both functions
//! and their ~10 call sites; these tests exercise a representative
//! cross-section: the equality-index plain-SELECT LIMIT push-down path
//! (`try_project_page_only_bytes` via `read_index_scan`), the sorted-index
//! range-scan general projection path (`apply_select_value_bytes` via
//! `read_sorted_index_scan`), and the `AsOf` temporal general path
//! (`apply_select_value_bytes` via `read_as_of`) — plus a no-false-positive
//! regression guard.

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::{At, Pagination, ReadQuery, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A non-MVCC table with an equality index on `status`. No MVCC store means
/// `read_index_scan`'s covering-gate is skipped entirely, landing directly
/// in the plain-SELECT (byte-level) branch that calls
/// `try_project_page_only_bytes` / `apply_select_value_bytes`.
async fn make_indexed_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();
    tbl.create_index("status_idx", &["status"]).await.unwrap();
    tbl
}

/// A non-MVCC table with a sorted index on `score` (non-covering — no
/// included fields), so `read_sorted_index_scan` falls straight to the
/// byte-level plain-SELECT branch.
async fn make_sorted_indexed_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();
    tbl.create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    tbl
}

/// An MVCC-backed table (required by `read_as_of`).
async fn make_mvcc_table() -> (TableManager, Arc<MvccStore>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    (tbl, mvcc)
}

/// Insert a valid `{status, name}` record through the normal encode path
/// (so any index on `status` gets a real posting) and return its id.
async fn insert_status_record(tbl: &TableManager, status: &str, name: &str) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let status_key = interner.touch_ind("status").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(status_key, InnerValue::Str(status.to_owned()));
    m.insert(name_key, InnerValue::Str(name.to_owned()));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Insert a valid `{score, label}` record and return its id.
async fn insert_score_record(tbl: &TableManager, score: i64, label: &str) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let score_key = interner.touch_ind("score").unwrap().into_key();
    let label_key = interner.touch_ind("label").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(score_key, InnerValue::Int(score));
    m.insert(label_key, InnerValue::Str(label.to_owned()));
    tbl.insert(&InnerValue::Map(m)).await.unwrap()
}

/// A lone msgpack bin8 marker (`0xc4`) with no length byte behind it fails
/// BOTH decode attempts the byte-level twins try: `RecordView::new` rejects
/// it (not a map-header byte), and the `InnerValue::from_bytes` fallback
/// also errors (`UnexpectedEof` — the bin8 marker demands a following length
/// byte that isn't there). This differs from `corrupt_record_tests.rs`'s
/// `\xff\xff\xff ...` fixture, which — while it correctly fails
/// `RecordView::new` (`0xff` is not a map-header byte) — decodes as a VALID
/// bare msgpack scalar (`-1`, a negative fixint) via the bare-scalar
/// `InnerValue::from_bytes` fallback these two functions uniquely have; a
/// fixture that "succeeds" at the fallback is not corrupt from this code
/// path's point of view and must not be reused here.
fn undecodable_bytes() -> bytes::Bytes {
    bytes::Bytes::from_static(b"\xc4")
}

/// Corrupt an ALREADY-INSERTED record's stored bytes in place (index
/// postings / MVCC hwm untouched — only the VALUE becomes undecodable),
/// mirroring `corrupt_record_tests.rs`'s injection technique but preserving
/// the index entry so the row is still a candidate the index scan matches.
async fn corrupt_in_place(tbl: &TableManager, id: RecordId) {
    tbl.data_store()
        .set(RecordKey::from_slice(id.as_bytes()), undecodable_bytes())
        .await
        .unwrap();
}

/// Corrupt a record's value in an MVCC-backed table by writing garbage
/// bytes directly through the `MvccStore`, bumping its version like a real
/// overwrite would.
async fn corrupt_in_place_mvcc(mvcc: &MvccStore, id: RecordId) {
    mvcc.set_versioned(RecordKey::from_slice(id.as_bytes()), undecodable_bytes())
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Equality-index plain-SELECT branch with a finite LIMIT — routes through
/// `read_index_scan` → `try_project_page_only_bytes` (Opt #3a LIMIT
/// push-down). A corrupt row alongside valid rows must not appear in
/// `records`, and `corrupt_records` must report it.
#[tokio::test]
async fn index_scan_limit_pushdown_reports_corrupt_record() {
    let tbl = make_indexed_table().await;
    let a = insert_status_record(&tbl, "active", "Alice").await;
    let b = insert_status_record(&tbl, "active", "Bob").await;
    let corrupt_id = insert_status_record(&tbl, "active", "Corrupt").await;
    corrupt_in_place(&tbl, corrupt_id).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .filter(Filter::Eq {
            field: FieldPath::from(vec!["status".to_string()]),
            value: FilterValue::String("active".into()),
        })
        .limit(10);
    let result = tbl.read(&query, &ctx).await.unwrap();

    let mut names: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("name").map(str::to_owned))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Alice".to_string(), "Bob".to_string()],
        "corrupt row must be excluded from records; valid rows unaffected"
    );

    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].table, "t");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
    let _ = a;
    let _ = b;
}

/// Sorted-index range scan (non-covering, no order_by/limit) — routes
/// through `read_sorted_index_scan`'s general path,
/// `apply_select_value_bytes`. A corrupt row inside the scanned range must
/// not appear in `records`, and `corrupt_records` must report it.
#[tokio::test]
async fn sorted_index_scan_reports_corrupt_record() {
    let tbl = make_sorted_indexed_table().await;
    insert_score_record(&tbl, 10, "a").await;
    insert_score_record(&tbl, 20, "b").await;
    let corrupt_id = insert_score_record(&tbl, 30, "c").await;
    corrupt_in_place(&tbl, corrupt_id).await;
    insert_score_record(&tbl, 40, "d").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t").filter(Filter::Between {
        field: FieldPath::from(vec!["score".to_string()]),
        from: FilterValue::Int(10),
        to: FilterValue::Int(40),
    });
    let result = tbl.read(&query, &ctx).await.unwrap();

    let mut labels: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("label").map(str::to_owned))
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        vec!["a".to_string(), "b".to_string(), "d".to_string()],
        "corrupt row (score=30) must be excluded; valid rows recover"
    );

    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].table, "t");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
}

/// `AsOf` temporal read, plain-SELECT general path — routes through
/// `read_as_of`'s `apply_select_value_bytes` call. A record whose AS-OF
/// value bytes are corrupt must be excluded from `records` (unchanged
/// behaviour) and reported in `corrupt_records`.
#[tokio::test]
async fn read_as_of_reports_corrupt_record() {
    let (tbl, mvcc) = make_mvcc_table().await;
    let valid_id = insert_score_record(&tbl, 10, "a").await;
    let corrupt_id = insert_score_record(&tbl, 20, "b").await;

    // Corrupt the second record's CURRENT (and therefore as-of) value.
    corrupt_in_place_mvcc(&mvcc, corrupt_id).await;
    let version = mvcc.live_version(corrupt_id.as_bytes()).unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut query = ReadQuery::new("t");
    query.temporal = Temporal::AsOf {
        at: At::Version(version),
    };
    let result = tbl.read(&query, &ctx).await.unwrap();

    let labels: Vec<String> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("label").map(str::to_owned))
        .collect();
    assert_eq!(
        labels,
        vec!["a".to_string()],
        "corrupt AS-OF value must be excluded; the valid record recovers"
    );

    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].table, "t");
    assert_eq!(result.corrupt_records[0].id, corrupt_id);
    let _ = valid_id;
}

/// Regression guard: no corrupt records injected → `corrupt_records` stays
/// empty on the byte-level index-scan path (no false positives introduced
/// by this task's plumbing).
#[tokio::test]
async fn index_scan_no_corrupt_records_when_all_rows_valid() {
    let tbl = make_indexed_table().await;
    insert_status_record(&tbl, "active", "Alice").await;
    insert_status_record(&tbl, "active", "Bob").await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .filter(Filter::Eq {
            field: FieldPath::from(vec!["status".to_string()]),
            value: FilterValue::String("active".into()),
        })
        .pagination(Pagination::LimitOffset {
            limit: Some(10),
            offset: 0,
        });
    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(result.records.len(), 2);
    assert!(
        result.corrupt_records.is_empty(),
        "no corrupt rows were injected — corrupt_records must be empty"
    );
}
