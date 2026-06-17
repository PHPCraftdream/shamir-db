//!
//! T4-history — the `History` temporal read.
//!
//! Validates the per-record version timeline returned by a `ReadQuery`
//! with `temporal: History { from, to, limit, order }`:
//!   - a record written several times yields one timeline row per
//!     version, ascending by `_version`, each carrying the projected
//!     field value that was current at that version;
//!   - `from`/`to` by `At::Version` filter the version range;
//!   - `limit` caps the WHOLE flattened result;
//!   - `order: Desc` reverses the timeline;
//!   - `AsOf` returns the clear "not yet supported (T4-asof)" error;
//!   - the `Latest` path is unchanged (regression).

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, OrderDirection, ReadQuery, Temporal};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build an MVCC-backed TableManager. No indexes — History uses the
/// current-state filter (a streaming scan) to pick records, then their
/// stored history.
async fn make_mvcc_table() -> (TableManager, Arc<MvccStore>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    // Default retention is CurrentOnly (max_count = 0): vacuum reclaims
    // every archived version immediately, leaving no timeline to read.
    // Opt into KeepHistory so prior versions survive.
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    (tbl, mvcc)
}

/// Intern the two field names used by the tests (`name`, `n`) and
/// persist the interner so subsequent reads resolve them.
async fn intern_fields(tbl: &TableManager) {
    let interner = tbl.interner().get().await.unwrap();
    let _ = interner.touch_ind("name").unwrap();
    let _ = interner.touch_ind("n").unwrap();
    tbl.interner().persist().await.unwrap();
}

/// Build an InnerValue `{name, n}` from pre-resolved interned keys.
fn build_record(
    name_key: &shamir_types::core::interner::InternerKey,
    n_key: &shamir_types::core::interner::InternerKey,
    name: &str,
    n: i64,
) -> InnerValue {
    let mut m = new_map();
    m.insert(name_key.clone(), InnerValue::Str(name.to_owned()));
    m.insert(n_key.clone(), InnerValue::Int(n));
    InnerValue::Map(m)
}

/// Insert a record and return its RecordId (v1).
async fn insert_first(tbl: &TableManager, rec: &InnerValue) -> RecordId {
    tbl.insert(rec).await.unwrap()
}

/// Overwrite an EXISTING RecordId with a new value via the MvccStore
/// directly — creates a new version (v2, v3, …) of the same key while
/// keeping the table's indexes pointing at the id. The bytes are the
/// serialised InnerValue (same layout `Table::insert` writes).
async fn overwrite(mvcc: &MvccStore, id: RecordId, rec: &InnerValue) {
    let bytes = rec.to_bytes().unwrap();
    mvcc.set_versioned(id.to_bytes(), bytes).await.unwrap();
}

/// `WHERE name == "alice"` — matches our subject record.
fn eq_name(name: &str) -> Filter {
    Filter::Eq {
        field: FieldPath::from(vec!["name".to_string()]),
        value: FilterValue::String(name.to_string()),
    }
}

/// `SELECT n FROM t WHERE name = "alice"` with a temporal selector.
fn history_query(name: &str, temporal: Temporal) -> ReadQuery {
    let mut q = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .filter(eq_name(name));
    q.temporal = temporal;
    q
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// A record written three times must yield three timeline rows with
/// `_version` ascending and the projected field `n` matching each
/// version's value.
#[tokio::test]
async fn history_returns_full_timeline_ascending() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let n_key = interner.touch_ind("n").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    // v1: {name: alice, n: 1}
    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    // v2: n = 2, v3: n = 3 (same RecordId).
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = history_query(
        "alice",
        Temporal::History {
            from: None,
            to: None,
            limit: None,
            order: OrderDirection::Asc,
        },
    );
    let result = tbl.read(&q, &ctx).await.unwrap();

    assert_eq!(
        result.records.len(),
        3,
        "three writes → three timeline rows"
    );

    let versions: Vec<u64> = result
        .records
        .iter()
        .filter_map(|r| r.get_owned("_version").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(versions, vec![1, 2, 3], "_version ascending 1,2,3");

    let ns: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_owned("n").and_then(|v| v.as_i64()))
        .collect();
    assert_eq!(ns, vec![1, 2, 3], "n matches each version's value");

    let index_used = result
        .stats
        .as_ref()
        .unwrap()
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        index_used.contains("temporal_history"),
        "index_used must be temporal_history; got {:?}",
        index_used
    );
}

/// `from: At::Version(2)` drops the first version; the remaining
/// timeline starts at v2.
#[tokio::test]
async fn history_from_version_filters_low_end() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let n_key = interner.touch_ind("n").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = history_query(
        "alice",
        Temporal::History {
            from: Some(At::Version(2)),
            to: None,
            limit: None,
            order: OrderDirection::Asc,
        },
    );
    let result = tbl.read(&q, &ctx).await.unwrap();

    let versions: Vec<u64> = result
        .records
        .iter()
        .filter_map(|r| r.get_owned("_version").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(versions, vec![2, 3], "from=Version(2) drops v1");
}

/// `limit` caps the WHOLE flattened result (documented): with limit=2
/// and Asc order, only the two oldest versions are returned.
#[tokio::test]
async fn history_limit_caps_whole_result() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let n_key = interner.touch_ind("n").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = history_query(
        "alice",
        Temporal::History {
            from: None,
            to: None,
            limit: Some(2),
            order: OrderDirection::Asc,
        },
    );
    let result = tbl.read(&q, &ctx).await.unwrap();

    let versions: Vec<u64> = result
        .records
        .iter()
        .filter_map(|r| r.get_owned("_version").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(versions, vec![1, 2], "limit=2 Asc keeps the two oldest");
}

/// `order: Desc` reverses the timeline — newest version first.
#[tokio::test]
async fn history_order_desc_reverses() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let n_key = interner.touch_ind("n").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = history_query(
        "alice",
        Temporal::History {
            from: None,
            to: None,
            limit: None,
            order: OrderDirection::Desc,
        },
    );
    let result = tbl.read(&q, &ctx).await.unwrap();

    let versions: Vec<u64> = result
        .records
        .iter()
        .filter_map(|r| r.get_owned("_version").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(versions, vec![3, 2, 1], "Desc newest-first");
}

/// `AsOf(Version(v))` on an MVCC table must succeed (T4-asof is now
/// implemented). An empty table with no records returns 0 rows — the
/// read succeeds and does NOT return an error or silently fall back to
/// Latest behaviour.
#[tokio::test]
async fn asof_returns_ok_on_mvcc_table() {
    let (tbl, _mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut q = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .filter(eq_name("alice"));
    q.temporal = Temporal::AsOf { at: At::Version(1) };

    // AsOf is implemented — must succeed, not error.
    let result = tbl.read(&q, &ctx).await.unwrap();
    // Empty table → 0 rows (no records existed at version 1 or at all).
    assert_eq!(
        result.records.len(),
        0,
        "AsOf on an empty table returns 0 rows"
    );
    // Stats tag must indicate the asof path was taken.
    let tag = result
        .stats
        .as_ref()
        .unwrap()
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        tag.contains("temporal_asof"),
        "index_used must be temporal_asof; got {:?}",
        tag
    );
}

/// Regression: the default `Latest` path is unchanged — a normal read
/// returns only the current value, with no `_version`/`_ts` fields and
/// no "temporal_history" stats tag.
#[tokio::test]
async fn latest_path_unchanged() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let n_key = interner.touch_ind("n").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Default temporal = Latest.
    let q = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .filter(eq_name("alice"));
    let result = tbl.read(&q, &ctx).await.unwrap();

    // Exactly one row — the current value.
    assert_eq!(result.records.len(), 1, "Latest returns one row");
    let n = result.records[0].get_i64("n").unwrap();
    assert_eq!(n, 3, "Latest returns the current value (n=3)");

    // No temporal metadata attached.
    assert!(
        result.records[0].get_owned("_version").is_none(),
        "Latest must not attach _version"
    );
    assert!(
        result.records[0].get_owned("_ts").is_none(),
        "Latest must not attach _ts"
    );

    // Index tag is NOT temporal_history.
    let index_used = result
        .stats
        .as_ref()
        .unwrap()
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        !index_used.contains("temporal_history"),
        "Latest must not report temporal_history; got {:?}",
        index_used
    );

    let _ = mvcc; // silence unused warning on mvcc binding
}

/// A non-MVCC table (no MvccStore attached) rejects History with a
/// clear error.
#[tokio::test]
async fn history_requires_mvcc_table() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();
    // NB: deliberately NO with_mvcc_store.
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q = history_query(
        "alice",
        Temporal::History {
            from: None,
            to: None,
            limit: None,
            order: OrderDirection::Asc,
        },
    );
    let err = tbl.read(&q, &ctx).await.unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("MVCC"),
                "error must mention MVCC; got {:?}",
                msg
            );
        }
        other => panic!("expected DbError::Validation, got {:?}", other),
    }
}
