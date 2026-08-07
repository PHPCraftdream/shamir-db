//! #1024 (follow-up to F-26 / #819) — `SelectItem::Expression` must
//! EVALUATE, not error, through every production read plan.
//!
//! `SelectProjection::new` (`crates/shamir-engine/src/query/read/
//! select_projection.rs`) is the single choke point every read plan funnels
//! its projection through. Before #1024 a computed `SelectItem::Expression`
//! was REJECTED here with `DbError::Validation("select_expression_not_
//! supported")` — this file used to assert that rejection. #1024 replaced
//! the reject with a real translation (`SelectExpr::to_filter_value`) into
//! the SAME `FilterValue`/`resolve_filter_query` pipeline `SelectItem::
//! Function` already uses. These tests now exercise the computed field
//! actually being evaluated through three DISTINCT production entry points
//! (full scan, index2/hash-index scan, temporal AsOf) rather than only the
//! shared `SelectProjection` constructor in isolation, proving every plan
//! type surfaces the same correct result AND that the old rejection error
//! no longer fires for any of them.

use std::sync::Arc;

use shamir_query_types::filter::{FieldPath, Filter, FilterValue};
use shamir_query_types::read::select::Select;
use shamir_query_types::read::{At, ReadQuery, SelectExpr, SelectExprValue, SelectItem, Temporal};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval_context::FilterContext;
use crate::table::TableManager;

/// A `SELECT age, (age + 1) AS bumped` item list — proves a real arithmetic
/// `SelectExpr::Add { Field, Literal }` tree, not just a bare literal.
fn select_with_expression() -> Select {
    Select {
        items: vec![
            SelectItem::Field {
                path: vec!["age".to_string()],
                alias: None,
            },
            SelectItem::Expression {
                expr: SelectExpr::Add {
                    left: Box::new(SelectExpr::Field {
                        path: vec!["age".to_string()],
                    }),
                    right: Box::new(SelectExpr::Literal {
                        value: SelectExprValue::Int(1),
                    }),
                },
                alias: Some("bumped".to_string()),
            },
        ],
        distinct: false,
    }
}

/// Assert every record in `records` has `bumped == age + 1` and that the
/// old `select_expression_not_supported` rejection did not fire (the caller
/// already unwrapped an `Ok(QueryResult)`, so this only re-checks values).
fn assert_expression_evaluated(records: &[shamir_query_types::read::QueryRecord]) {
    assert!(!records.is_empty(), "expected at least one row");
    for r in records {
        let qv = r.as_value();
        let age = match &qv["age"] {
            QueryValue::Int(i) => *i,
            other => panic!("expected age: Int, got {other:?}"),
        };
        let bumped = match &qv["bumped"] {
            QueryValue::Int(i) => *i,
            other => panic!("expected bumped: Int, got {other:?}"),
        };
        assert_eq!(bumped, age + 1, "bumped must equal age + 1");
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
async fn full_scan_evaluates_select_expression() {
    let tbl = make_table().await;
    seed_rows(&tbl).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("t").select(select_with_expression());
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_expression_evaluated(&result.records);
}

// ─────────────────────────────────────────────────────────────────────────
// index2 / hash-index scan (`read_index_scan.rs::read_index_scan`) —
// an Eq filter on an indexed field routes through the index-scan plan
// regardless of the SELECT shape (`try_plan_index_scan` only inspects
// `query.r#where` + the interner, never `query.select`).
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn index2_scan_evaluates_select_expression() {
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
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_expression_evaluated(&result.records);
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].as_value()["bumped"], QueryValue::Int(31));
}

// ─────────────────────────────────────────────────────────────────────────
// Temporal AsOf (`read_temporal.rs::read_as_of`) — an MVCC-backed table's
// point-in-time read.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn temporal_asof_evaluates_select_expression() {
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
    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_expression_evaluated(&result.records);
}
