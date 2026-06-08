//!
//! T4-asof — `AsOf` point-in-time temporal read tests.
//!
//! Validates that `ReadQuery { temporal: AsOf(At) }` returns the table
//! state as it existed at the given version, that records not yet created
//! at that version are excluded, that the WHERE filter runs on the as-of
//! values (not current values), that `AsOf(Timestamp)` resolves or errors
//! clearly, and that `Latest` / `History` paths are unaffected.

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, ReadQuery, Temporal};
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
// Helpers (mirror history_read_tests.rs)
// ─────────────────────────────────────────────────────────────────────────────

async fn make_mvcc_table() -> (TableManager, Arc<MvccStore>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let base = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    // Keep full history so prior versions survive vacuum.
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));
    (tbl, mvcc)
}

async fn intern_fields(tbl: &TableManager) {
    let interner = tbl.interner().get().await.unwrap();
    let _ = interner.touch_ind("name").unwrap();
    let _ = interner.touch_ind("n").unwrap();
    tbl.interner().persist().await.unwrap();
}

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

async fn insert_first(tbl: &TableManager, rec: &InnerValue) -> RecordId {
    tbl.insert(rec).await.unwrap()
}

async fn overwrite(mvcc: &MvccStore, id: RecordId, rec: &InnerValue) -> u64 {
    let bytes = rec.to_bytes().unwrap();
    mvcc.set_versioned(id.to_bytes(), bytes).await.unwrap()
}

fn eq_name(name: &str) -> Filter {
    Filter::Eq {
        field: FieldPath::from(vec!["name".to_string()]),
        value: FilterValue::String(name.to_string()),
    }
}

fn eq_n(n: i64) -> Filter {
    Filter::Eq {
        field: FieldPath::from(vec!["n".to_string()]),
        value: FilterValue::Int(n),
    }
}

fn asof_query(temporal: Temporal) -> ReadQuery {
    ReadQuery::new("t")
        .select(Select::fields(["n", "name"]))
        .into_with_temporal(temporal)
}

trait IntoWithTemporal {
    fn into_with_temporal(self, temporal: Temporal) -> Self;
}

impl IntoWithTemporal for ReadQuery {
    fn into_with_temporal(mut self, temporal: Temporal) -> Self {
        self.temporal = temporal;
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: AsOf(Version(v1)) returns the v1 state; Latest returns v2.
// ─────────────────────────────────────────────────────────────────────────────

/// A record inserted at v1, then updated at v2. `AsOf(Version(v1))` must
/// return the v1 value; `Latest` must return v2. If `read_as_of` is broken
/// (e.g. it reads current values instead of as-of values) the assertion on
/// `n == 1` would fail because the current value has `n = 2`.
#[tokio::test]
async fn asof_version_returns_state_at_that_version() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().key().clone();
    let n_key = interner.touch_ind("n").unwrap().key().clone();
    tbl.interner().persist().await.unwrap();

    // v1: {name: alice, n: 1}
    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    let v1 = mvcc.version_of(&id.to_bytes());
    // v2: {name: alice, n: 2}
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // AsOf(v1) → must see n = 1
    let mut q = asof_query(Temporal::AsOf {
        at: At::Version(v1),
    });
    q.r#where = Some(eq_name("alice"));
    let result = tbl.read(&q, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 1, "AsOf(v1) returns one row");
    let n = result.records[0].get("n").and_then(|v| v.as_i64()).unwrap();
    assert_eq!(n, 1, "AsOf(v1) must return n=1 (the v1 value)");

    // Latest → must see n = 2
    let mut q2 = ReadQuery::new("t").select(Select::fields(["n"]));
    q2.r#where = Some(eq_name("alice"));
    let res2 = tbl.read(&q2, &ctx).await.unwrap();
    assert_eq!(res2.records.len(), 1, "Latest returns one row");
    let n2 = res2.records[0].get("n").and_then(|v| v.as_i64()).unwrap();
    assert_eq!(n2, 2, "Latest must return n=2 (the current value)");

    // Stats tag must be temporal_asof.
    let tag = result
        .stats
        .as_ref()
        .unwrap()
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        tag.contains("temporal_asof"),
        "index_used must contain temporal_asof; got {:?}",
        tag
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: AsOf excludes records not yet created at that version.
// ─────────────────────────────────────────────────────────────────────────────

/// Record A is created at vA; Record B is created at vB > vA.
/// `AsOf(Version(vA))` must include A but EXCLUDE B.
/// If `read_as_of` were broken (returning current-state records) it would
/// include both A and B.
#[tokio::test]
async fn asof_excludes_records_not_yet_created() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().key().clone();
    let n_key = interner.touch_ind("n").unwrap().key().clone();
    tbl.interner().persist().await.unwrap();

    // Insert A, capture vA.
    let id_a = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    let v_a = mvcc.version_of(&id_a.to_bytes());

    // Insert B (vB > vA by monotonic counter).
    let _id_b = insert_first(&tbl, &build_record(&name_key, &n_key, "bob", 2)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // AsOf(vA): should see A, must NOT see B.
    let q = ReadQuery::new("t")
        .select(Select::fields(["name", "n"]))
        .into_with_temporal(Temporal::AsOf {
            at: At::Version(v_a),
        });
    let result = tbl.read(&q, &ctx).await.unwrap();

    assert_eq!(
        result.records.len(),
        1,
        "AsOf(vA) must return exactly 1 row (A only)"
    );
    let name = result.records[0]
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        name, "alice",
        "AsOf(vA) must return alice, not bob (bob did not exist yet)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: WHERE filter runs on as-of values, not current values.
// ─────────────────────────────────────────────────────────────────────────────

/// A record with n=1 at v_old is updated to n=99 at v_new.
/// `WHERE n == 1` under `AsOf(Version(v_old))` MUST match.
/// `WHERE n == 1` under `Latest` MUST NOT match.
///
/// If `read_as_of` runs the WHERE on current values (n=99) the AsOf query
/// would return 0 rows; the test catches this.
#[tokio::test]
async fn asof_where_filter_on_as_of_value() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().key().clone();
    let n_key = interner.touch_ind("n").unwrap().key().clone();
    tbl.interner().persist().await.unwrap();

    // v_old: {name: alice, n: 1}
    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    let v_old = mvcc.version_of(&id.to_bytes());

    // v_new: {name: alice, n: 99}
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 99)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // AsOf(v_old) WHERE n == 1 → must return 1 row (n was 1 then).
    let mut q_asof = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .into_with_temporal(Temporal::AsOf {
            at: At::Version(v_old),
        });
    q_asof.r#where = Some(eq_n(1));
    let res_asof = tbl.read(&q_asof, &ctx).await.unwrap();
    assert_eq!(
        res_asof.records.len(),
        1,
        "AsOf(v_old) WHERE n==1 must match (n was 1 at that version)"
    );

    // Latest WHERE n == 1 → must return 0 rows (n is now 99).
    let mut q_latest = ReadQuery::new("t").select(Select::fields(["n"]));
    q_latest.r#where = Some(eq_n(1));
    let res_latest = tbl.read(&q_latest, &ctx).await.unwrap();
    assert_eq!(
        res_latest.records.len(),
        0,
        "Latest WHERE n==1 must not match (current n=99)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: AsOf(Timestamp) resolves to a version or returns a clear error.
// ─────────────────────────────────────────────────────────────────────────────

/// `AsOf(Timestamp(t))` resolves to the correct version when a committed
/// version has a recorded ts ≤ t.  It must NOT silently fall back to Latest.
///
/// We use the MvccStore clock: write a record, then query with a timestamp
/// larger than "now" — this should resolve to the written version. If there
/// is no recorded ts (or the timestamp is before any version) we expect a
/// clear validation error.
///
/// Two sub-cases:
/// a) timestamp ≥ the first commit's recorded ts → resolves, returns n=1.
/// b) timestamp = 0 (before any commit) → clear error (no version ≤ ts=0).
#[tokio::test]
async fn asof_timestamp_resolves_or_errors() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().key().clone();
    let n_key = interner.touch_ind("n").unwrap().key().clone();
    tbl.interner().persist().await.unwrap();

    // Write a record so at least one ts-key exists.
    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    let _ = id;

    // Sub-case b: ts = 0 — no version can have ts ≤ 0 (real timestamps are
    // millis since UNIX_EPOCH, so >> 0).
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let q_ts0 = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .into_with_temporal(Temporal::AsOf {
            at: At::Timestamp(0),
        });
    let err = tbl.read(&q_ts0, &ctx).await.unwrap_err();
    match err {
        DbError::Validation(msg) => {
            assert!(
                msg.contains("AsOf(Timestamp("),
                "error must mention AsOf(Timestamp(; got {:?}",
                msg
            );
            assert!(
                !msg.to_lowercase().contains("not yet supported"),
                "must NOT say 'not yet supported'; got {:?}",
                msg
            );
        }
        other => panic!("expected DbError::Validation, got {:?}", other),
    }

    // Sub-case a: ts = u64::MAX — resolves to the most recent version.
    // A timestamp larger than any real clock value always resolves to the
    // last committed version; we just verify the query succeeds and returns
    // alice (the only record).
    let q_big = ReadQuery::new("t")
        .select(Select::fields(["name"]))
        .into_with_temporal(Temporal::AsOf {
            at: At::Timestamp(u64::MAX),
        });
    let res = tbl.read(&q_big, &ctx).await.unwrap();
    assert_eq!(
        res.records.len(),
        1,
        "AsOf(Timestamp(MAX)) must resolve and return the record"
    );
    let name = res.records[0]
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(name, "alice");

    let _ = mvcc;
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5: Latest and History paths are unchanged.
// ─────────────────────────────────────────────────────────────────────────────

/// Regression guard: after introducing `read_as_of`, both the default
/// `Latest` path and the `History` path must continue to work exactly as
/// before. A Latest query returns only the current value (no temporal meta);
/// a History query returns the full version timeline.
#[tokio::test]
async fn latest_and_history_unchanged() {
    let (tbl, mvcc) = make_mvcc_table().await;
    intern_fields(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().key().clone();
    let n_key = interner.touch_ind("n").unwrap().key().clone();
    tbl.interner().persist().await.unwrap();

    let id = insert_first(&tbl, &build_record(&name_key, &n_key, "alice", 1)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 2)).await;
    overwrite(&mvcc, id, &build_record(&name_key, &n_key, "alice", 3)).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // ── Latest ───────────────────────────────────────────────────────────────
    let q_latest = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .filter(Filter::Eq {
            field: FieldPath::from(vec!["name".to_string()]),
            value: FilterValue::String("alice".to_string()),
        });
    let res_latest = tbl.read(&q_latest, &ctx).await.unwrap();
    assert_eq!(res_latest.records.len(), 1, "Latest returns one row");
    let n = res_latest.records[0]
        .get("n")
        .and_then(|v| v.as_i64())
        .unwrap();
    assert_eq!(n, 3, "Latest must return current value n=3");
    assert!(
        res_latest.records[0].get("_version").is_none(),
        "Latest must not attach _version"
    );

    // ── History ──────────────────────────────────────────────────────────────
    let mut q_hist = ReadQuery::new("t")
        .select(Select::fields(["n"]))
        .filter(Filter::Eq {
            field: FieldPath::from(vec!["name".to_string()]),
            value: FilterValue::String("alice".to_string()),
        });
    q_hist.temporal = Temporal::History {
        from: None,
        to: None,
        limit: None,
        order: shamir_query_types::read::OrderDirection::Asc,
    };
    let res_hist = tbl.read(&q_hist, &ctx).await.unwrap();
    assert_eq!(
        res_hist.records.len(),
        3,
        "History must return all 3 versions"
    );
    let versions: Vec<u64> = res_hist
        .records
        .iter()
        .filter_map(|r| r.get("_version").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(versions, vec![1, 2, 3], "History versions ascending 1,2,3");

    let hist_tag = res_hist
        .stats
        .as_ref()
        .unwrap()
        .index_used
        .as_deref()
        .unwrap_or("");
    assert!(
        hist_tag.contains("temporal_history"),
        "History must report temporal_history; got {:?}",
        hist_tag
    );
}
