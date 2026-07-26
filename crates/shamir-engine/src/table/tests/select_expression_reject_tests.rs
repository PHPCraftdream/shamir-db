//! F-26 (#819) — `SelectItem::Expression` must be REJECTED, not silently
//! dropped, by every production read plan.
//!
//! `SelectProjection::new` (`crates/shamir-engine/src/query/read/
//! select_projection.rs`) is the single choke point every read plan funnels
//! its projection through — but a bare `_ => {}` catch-all used to let a
//! computed `SelectItem::Expression` fall through silently, returning a
//! result row with the field simply absent instead of an error. These tests
//! exercise the reject through three DISTINCT production entry points (full
//! scan, index2/hash-index scan, temporal AsOf) rather than only the shared
//! `SelectProjection` constructor in isolation, proving every plan type
//! actually surfaces the same typed rejection.

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, ReadQuery, SelectExpr, SelectExprValue, SelectItem, Temporal};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

/// A `SELECT age, <computed expr> AS bogus` item list — the `Expression`
/// arm is a scaffolded-but-never-finished variant (see `select.rs`'s doc
/// comment); its exact expression shape doesn't matter for this test, only
/// that `SelectItem::Expression` is present in `select.items`.
fn select_with_expression() -> Select {
    Select {
        items: vec![
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: None,
            },
            SelectItem::Expression {
                expr: SelectExpr::Literal {
                    value: SelectExprValue::Int(1),
                },
                alias: Some("bogus".to_string()),
            },
        ],
        distinct: false,
    }
}

/// Assert `err` is `DbError::Validation` carrying the stable
/// `select_expression_not_supported` code (the SAME code the plain
/// projection reject and the aggregate-path reject both use).
fn assert_rejects_select_expression(err: DbError) {
    match err {
        DbError::Validation(msg) => assert!(
            msg.contains("select_expression_not_supported"),
            "expected the select_expression_not_supported code, got: {msg}"
        ),
        other => panic!("expected DbError::Validation, got {other:?}"),
    }
}

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

async fn seed_rows(tbl: &TableManager) -> Vec<shamir_types::types::record_id::RecordId> {
    let interner = tbl.interner().get().await.unwrap();
    let age_k = interner.touch_ind("age").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut ids = Vec::new();
    for age in [20_i64, 30, 40] {
        let mut m = new_map();
        m.insert(age_k.clone(), InnerValue::Int(age));
        ids.push(tbl.insert(&InnerValue::Map(m)).await.unwrap());
    }
    ids
}

// ─────────────────────────────────────────────────────────────────────────
// Full scan (read_collecting / read_streaming / read_counting, no index,
// no WHERE eligible for an index scan).
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_scan_rejects_select_expression() {
    let tbl = make_table().await;
    seed_rows(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t").select(select_with_expression());
    let err = tbl.read(&query, &ctx).await.unwrap_err();
    assert_rejects_select_expression(err);
}

// ─────────────────────────────────────────────────────────────────────────
// index2 / hash-index scan (`read_index_scan.rs::read_index_scan`) —
// an Eq filter on an indexed field routes through the index-scan plan
// regardless of the SELECT shape (`try_plan_index_scan` only inspects
// `query.r#where` + the interner, never `query.select`).
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn index2_scan_rejects_select_expression() {
    let tbl = make_table().await;
    seed_rows(&tbl).await;
    tbl.create_index("age_idx", &["age"]).await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t")
        .select(select_with_expression())
        .filter(Filter::Eq {
            field: FieldPath::from(vec!["age".to_string()]),
            value: FilterValue::Int(30),
        });
    let err = tbl.read(&query, &ctx).await.unwrap_err();
    assert_rejects_select_expression(err);
}

// ─────────────────────────────────────────────────────────────────────────
// Temporal AsOf (`read_temporal.rs::read_as_of`) — an MVCC-backed table's
// point-in-time read.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn temporal_asof_rejects_select_expression() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let base = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();
    let tbl = base.with_mvcc_store(Arc::clone(&mvcc));

    let ids = seed_rows(&tbl).await;
    let v1 = mvcc.version_of(&ids[0].to_bytes());

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let mut query = ReadQuery::new("t").select(select_with_expression());
    query.temporal = Temporal::AsOf {
        at: At::Version(v1),
    };
    let err = tbl.read(&query, &ctx).await.unwrap_err();
    assert_rejects_select_expression(err);
}
