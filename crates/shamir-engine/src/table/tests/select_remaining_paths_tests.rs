//! Remaining SELECT paths must retain batch refs and fresh ForEach params.

use std::sync::Arc;

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::{filter, select, val, Query};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::batch::{execute_batch_unchecked, TableResolver};
use crate::query::read::QueryResult;
use crate::query::TableRef;
use crate::repo::RepoInstance;
use crate::table::TableManager;

/// The legacy wrapper has no batch bindings, even for nested SELECT arguments.
/// Previously these queries succeeded with Null (or hid bindings in unused branches).
#[test]
fn legacy_select_rejects_nested_external_bindings() {
    let interner = shamir_types::core::interner::Interner::default();
    let records = vec![(
        shamir_types::types::record_id::RecordId::new(),
        InnerValue::Null,
    )];
    for binding in [val::qref("q1", "[0].m"), val::param("row")] {
        let arguments = [
            val::add(val::lit(1), binding.clone()),
            val::lit(vec![binding.clone()]),
            val::func("math/max", [binding.clone()]),
            val::cond(filter::eq("v", 1), binding.clone(), val::lit(0)),
            val::cond(filter::eq("v", 1), val::lit(0), binding.clone()),
            val::cond(filter::eq("v", binding), val::lit(1), val::lit(0)),
        ];
        for argument in arguments {
            let query = Query::from("rows")
                .select([select::func("total", "math/max", [argument])])
                .build();
            for input in [records.as_slice(), &[]] {
                let error = crate::query::read::exec::apply_select_value(
                    input,
                    &query.select,
                    &interner,
                    ScalarResolver::builtins_only(),
                )
                .unwrap_err();
                assert!(matches!(
                    error,
                    DbError::Validation(ref code) if code == "select_bindings_require_context"
                ));
            }
        }
    }
}

#[test]
fn legacy_select_preserves_fields_and_null_fallbacks() {
    let interner = shamir_types::core::interner::Interner::default();
    let mut map = new_map();
    map.insert(
        interner.touch_ind("v").unwrap().into_key(),
        InnerValue::Int(3),
    );
    let records = vec![(
        shamir_types::types::record_id::RecordId::new(),
        InnerValue::Map(map),
    )];
    let query = Query::from("rows")
        .select([
            select::field("v"),
            select::func("total", "math/max", [val::add(val::col("v"), val::lit(7))]),
            select::func("missing", "math/max", [val::col("absent")]),
            select::func("unknown", "missing/scalar", [val::col("v")]),
        ])
        .build();
    let result = crate::query::read::exec::apply_select_value(
        &records,
        &query.select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
    let QueryValue::Map(row) = &result[0] else {
        panic!("expected projected map");
    };
    assert_eq!(row["v"].as_i64(), Some(3));
    assert_eq!(row["total"].as_i64(), Some(10));
    assert_eq!(row["missing"], QueryValue::Null);
    assert_eq!(row["unknown"], QueryValue::Null);
}

struct Resolver {
    table: TableManager,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        assert_eq!(table_ref.table, "rows");
        Ok(self.table.clone())
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        // Read-only batches use the standalone table's MVCC store, not a repo drainer.
        Err(DbError::NotFound("standalone fixture has no repo".into()))
    }
}

async fn setup(temporal: bool) -> (Resolver, Option<Arc<MvccStore>>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut table = TableManager::create("rows".into(), data, info)
        .await
        .unwrap();
    let mvcc = if temporal {
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let store = Arc::new(MvccStore::new(history, Arc::new(RepoTxGate::fresh())));
        store.set_retention(Retention::keep_history()).unwrap();
        table = table.with_mvcc_store(Arc::clone(&store));
        Some(store)
    } else {
        None
    };
    (Resolver { table }, mvcc)
}

async fn record(table: &TableManager, v: i64) -> InnerValue {
    let interner = table.interner().get().await.unwrap();
    let mut map = new_map();
    for (field, value) in [("v", v), ("m", 7), ("score", 10)] {
        map.insert(
            interner.touch_ind(field).unwrap().into_key(),
            InnerValue::Int(value),
        );
    }
    table.interner().persist().await.unwrap();
    InnerValue::Map(map)
}

fn projection(query: Query, params: bool) -> Query {
    let mut items = vec![
        select::field("v"),
        select::func(
            "total",
            "math/max",
            [val::add(val::col("v"), val::qref("q1", "[0].m"))],
        ),
    ];
    if params {
        items.push(select::func("tag", "math/max", [val::param("row")]));
    }
    query.select(items)
}

fn assert_path(result: &QueryResult, path: &str, returned: u64) {
    let stats = result.stats.as_ref().unwrap();
    assert_eq!(stats.index_used.as_deref(), Some(path));
    assert_eq!(stats.records_scanned, 1);
    assert_eq!(stats.records_returned, returned);
    assert!(result.corrupt_records.is_empty());
}

/// Counterfactual: without context binding, total is Null rather than v + 7,
/// and ForEach tags are Null. Reusing the first parameter binding yields [1, 1].
async fn check_batch_and_foreach(
    resolver: &Resolver,
    query: Query,
    path: &str,
    expected_values: &[i64],
    versions: &[u64],
) {
    let mut batch = Batch::new();
    // Consumer first: only the SELECT dependency may stage it after q1.
    batch.query("q2", projection(query.clone(), false));
    batch.query("q1", Query::from("rows").select([select::field("m")]));
    let response =
        execute_batch_unchecked(&batch.build(), resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    assert_eq!(response.results["q1"].records.len(), 1);
    assert_eq!(
        response.results["q1"].records[0].get_value_i64("m"),
        Some(7)
    );
    let result = &response.results["q2"];
    assert_path(result, path, u64::try_from(expected_values.len()).unwrap());
    assert_eq!(result.records.len(), expected_values.len());
    for (index, expected) in expected_values.iter().enumerate() {
        let row = &result.records[index];
        assert_eq!(row.get_value_i64("v"), Some(*expected));
        assert_eq!(row.get_value_i64("total"), Some(*expected + 7));
        if !versions.is_empty() {
            assert_eq!(row.get_value_u64("_version"), Some(versions[index]));
        }
    }

    let mut body = Batch::new();
    body.query("q2", projection(query, true));
    body.query("q1", Query::from("rows").select([select::field("m")]));
    let mut outer = Batch::new();
    outer.for_each("loop", vec![val::lit(1), val::lit(2)], "row", body.build());
    let response =
        execute_batch_unchecked(&outer.build(), resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    let QueryValue::List(iterations) = response.results["loop"].value.as_ref().unwrap() else {
        panic!("expected iteration list");
    };
    assert_eq!(iterations.len(), 2);
    for (index, tag) in [1, 2].into_iter().enumerate() {
        let QueryValue::Map(results) = &iterations[index] else {
            panic!("expected result map");
        };
        let QueryValue::Map(result) = &results["q2"] else {
            panic!("expected query result");
        };
        let QueryValue::List(rows) = &result["records"] else {
            panic!("expected records");
        };
        assert_eq!(rows.len(), expected_values.len());
        for (row_index, expected) in expected_values.iter().enumerate() {
            let QueryValue::Map(row) = &rows[row_index] else {
                panic!("expected projected row");
            };
            assert_eq!(row["v"], QueryValue::Int(*expected));
            assert_eq!(row["total"], QueryValue::Int(*expected + 7));
            assert_eq!(row["tag"], QueryValue::Int(tag));
            if !versions.is_empty() {
                assert_eq!(
                    row["_version"],
                    QueryValue::Int(i64::try_from(versions[row_index]).unwrap()),
                );
            }
        }
    }
}

#[tokio::test]
async fn equality_index_select_context_general_and_limit() {
    let (resolver, _) = setup(false).await;
    resolver.table.create_index("v_idx", &["v"]).await.unwrap();
    resolver
        .table
        .insert(&record(&resolver.table, 3).await)
        .await
        .unwrap();
    for limited in [false, true] {
        let mut query = Query::from("rows").where_eq("v", 3);
        if limited {
            query = query.limit(1);
        }
        check_batch_and_foreach(&resolver, query, "v_idx", &[3], &[]).await;
    }
}

#[tokio::test]
async fn sorted_index_select_context_range_limit_order_and_seek() {
    let (resolver, _) = setup(false).await;
    resolver
        .table
        .create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    resolver
        .table
        .insert(&record(&resolver.table, 3).await)
        .await
        .unwrap();
    let index = resolver.table.sorted_indexes().iter_indexes()[0].name_interned;
    for limited in [false, true] {
        let mut query = Query::from("rows").where_between("score", 0, 20);
        if limited {
            query = query.limit(1);
        }
        check_batch_and_foreach(&resolver, query, &format!("sorted_idx_{index}"), &[3], &[]).await;
    }
    check_batch_and_foreach(
        &resolver,
        Query::from("rows").order_by_asc("score").limit(1),
        &format!("sorted_idx_{index}_first_k"),
        &[3],
        &[],
    )
    .await;
    check_batch_and_foreach(
        &resolver,
        Query::from("rows")
            .order_by_asc("score")
            .after(vec![QueryValue::Int(0)], Some(1)),
        &format!("sorted_idx_{index}_keyset"),
        &[3],
        &[],
    )
    .await;
}

#[tokio::test]
async fn asof_select_context_general_limit_and_topk() {
    let (resolver, mvcc) = setup(true).await;
    let mvcc = mvcc.unwrap();
    let id = resolver
        .table
        .insert(&record(&resolver.table, 3).await)
        .await
        .unwrap();
    let old = mvcc.version_of(id.as_bytes());
    mvcc.set_versioned(
        RecordKey::from_slice(id.as_bytes()),
        record(&resolver.table, 5).await.to_bytes().unwrap(),
    )
    .await
    .unwrap();
    for mode in 0..3 {
        let mut query = Query::from("rows").as_of_version(old);
        if mode == 1 {
            query = query.limit(1);
        } else if mode == 2 {
            // No sorted index: ORDER BY + LIMIT must use the inline temporal heap.
            query = query.order_by_asc("v").limit(1);
        }
        check_batch_and_foreach(&resolver, query, "temporal_asof", &[3], &[]).await;
    }
}

#[tokio::test]
async fn asof_keyset_select_context() {
    let (resolver, mvcc) = setup(true).await;
    let mvcc = mvcc.unwrap();
    resolver
        .table
        .create_sorted_index("score_idx", &["score"])
        .await
        .unwrap();
    let id = resolver
        .table
        .insert(&record(&resolver.table, 3).await)
        .await
        .unwrap();
    let version = mvcc.version_of(id.as_bytes());
    let index = resolver.table.sorted_indexes().iter_indexes()[0].name_interned;
    check_batch_and_foreach(
        &resolver,
        Query::from("rows")
            .as_of_version(version)
            .order_by_asc("score")
            .after(vec![QueryValue::Int(0)], Some(1)),
        &format!("sorted_idx_{index}_asof_keyset"),
        &[3],
        &[],
    )
    .await;
}

#[tokio::test]
async fn history_select_context_projects_each_version() {
    let (resolver, mvcc) = setup(true).await;
    let mvcc = mvcc.unwrap();
    let id = resolver
        .table
        .insert(&record(&resolver.table, 3).await)
        .await
        .unwrap();
    let old = mvcc.version_of(id.as_bytes());
    let new = mvcc
        .set_versioned(
            RecordKey::from_slice(id.as_bytes()),
            record(&resolver.table, 5).await.to_bytes().unwrap(),
        )
        .await
        .unwrap();
    check_batch_and_foreach(
        &resolver,
        Query::from("rows").history(),
        "temporal_history",
        &[3, 5],
        &[old, new],
    )
    .await;
}
