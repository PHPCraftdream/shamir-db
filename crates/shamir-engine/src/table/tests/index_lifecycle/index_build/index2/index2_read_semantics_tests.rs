//! B03 differential semantics. Red reasons are documented at each case.

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::{ddl, filter as qf, select, Query};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::filter::Filter;
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::FilterContext;
use crate::query::read::QueryResult;
use crate::table::TableManager;

async fn fixture(indexed: bool, vector: bool, versioned: bool) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut table = TableManager::create("rows".into(), data, info)
        .await
        .unwrap();
    if versioned {
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        table = table.with_mvcc_store(Arc::new(MvccStore::new(
            history,
            Arc::new(RepoTxGate::fresh()),
        )));
    }
    for (title, amount, embedding) in [
        ("z", 30, [1.0, 0.0]),
        ("a", 10, [0.8, 0.2]),
        ("a", 20, [0.0, 1.0]),
    ] {
        let interner = table.interner().get().await.unwrap();
        let mut row = new_map();
        for (name, value) in [
            ("body", InnerValue::Str("hello world".into())),
            ("title", InnerValue::Str(title.into())),
            ("amount", InnerValue::Int(amount)),
            ("tag", InnerValue::Str("yes".into())),
            (
                "embedding",
                InnerValue::List(embedding.into_iter().map(InnerValue::F64).collect()),
            ),
        ] {
            row.insert(interner.touch_ind(name).unwrap().into_key(), value);
        }
        table.interner().persist().await.unwrap();
        table.insert(&InnerValue::Map(row)).await.unwrap();
    }
    if indexed {
        let builder = ddl::create_index("search", "rows");
        let builder = if vector {
            builder
                .field("embedding")
                .index_type("vector")
                .vector_dim(2)
                .vector_metric("cosine")
        } else {
            builder.field("body").index_type("fts")
        };
        let BatchOp::CreateIndex(op) = builder.build() else {
            panic!("expected CREATE INDEX")
        };
        table.create_index_v2(&op).await.unwrap();
    }
    table
}

async fn read(table: &TableManager, query: Query, filter: Filter) -> QueryResult {
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    table
        .read(&query.build().filter(filter), &ctx)
        .await
        .unwrap()
}

fn values(result: &QueryResult) -> Vec<QueryValue> {
    result
        .records
        .iter()
        .map(|r| r.as_value().into_owned())
        .collect()
}

async fn differential(query: Query, filter: Filter, vector: bool, ordered: bool) -> QueryResult {
    let indexed = fixture(true, vector, false).await;
    let unindexed = fixture(false, vector, false).await;
    let actual = read(&indexed, query.clone(), filter.clone()).await;
    let expected = read(&unindexed, query, filter).await;
    let mut a = values(&actual);
    let mut b = values(&expected);
    if !ordered {
        // Natural store traversal and ANN rank are not the same ordering contract.
        a.sort_by_key(|v| format!("{v:?}"));
        b.sort_by_key(|v| format!("{v:?}"));
    }
    assert_eq!(a, b);
    assert_eq!(actual.corrupt_records.len(), expected.corrupt_records.len());
    assert_eq!(actual.pagination, expected.pagination);
    actual
}

fn fts() -> Filter {
    qf::fts("body", "hello", "and")
}

fn vector(filtered: bool) -> Filter {
    // Limitation: unindexed vector predicates are True, not ANN. Top-k selects
    // ALL qualifying fixture rows, so this oracle tests postprocessing only,
    // not ANN recall or equivalence for a strict top-k subset.
    let v = qf::vector_similarity("embedding", vec![1.0, 0.0], 3);
    if filtered {
        qf::and([v, qf::eq("tag", "yes")])
    } else {
        v
    }
}

#[tokio::test]
async fn fts_projection() {
    // Old shortcut returns entire records instead of SELECT title.
    let r = differential(Query::from("rows").select(["title"]), fts(), false, false).await;
    for value in values(&r) {
        let QueryValue::Map(map) = value else {
            panic!("expected map")
        };
        assert_eq!(map.len(), 1);
    }
}

#[tokio::test]
async fn fts_count_and_sum() {
    // Old shortcut returns three raw rows, never reducing COUNT or SUM.
    for (item, alias, expected) in [
        (select::count_all("count"), "count", 3),
        (select::sum("amount", "sum"), "sum", 60),
    ] {
        let r = differential(Query::from("rows").select([item]), fts(), false, true).await;
        let v = values(&r);
        assert_eq!(v.len(), 1);
        let QueryValue::Map(map) = &v[0] else {
            panic!("expected map")
        };
        assert_eq!(map[alias], QueryValue::Int(expected));
    }
}

#[tokio::test]
async fn fts_distinct() {
    // Old shortcut neither projects nor removes duplicate titles.
    let r = differential(
        Query::from("rows").select(["title"]).distinct(),
        fts(),
        false,
        false,
    )
    .await;
    assert_eq!(r.records.len(), 2);
}

#[tokio::test]
async fn fts_order_limit_offset() {
    // Pick a sort whose requested page MUST disagree with candidate RID order,
    // even when all FTS scores tie. Use the same indexed fixture for the probe.
    let indexed = fixture(true, false, false).await;
    let unindexed = fixture(false, false, false).await;
    let full = read(&indexed, Query::from("rows"), fts()).await;
    let full_values = values(&full);
    let QueryValue::Map(last) = &full_values[2] else {
        panic!("expected map")
    };
    let (query, amount) = if last["amount"] == QueryValue::Int(30) {
        (Query::from("rows").order_by_desc("amount"), 10)
    } else {
        (Query::from("rows").order_by_asc("amount"), 30)
    };
    let query = query.offset(2).limit(1);
    let actual = read(&indexed, query.clone(), fts()).await;
    let expected = read(&unindexed, query, fts()).await;
    assert_eq!(values(&actual), values(&expected));
    assert_eq!(actual.pagination, expected.pagination);
    let rows = values(&actual);
    assert_eq!(rows.len(), 1);
    let QueryValue::Map(row) = &rows[0] else {
        panic!("expected map")
    };
    assert_eq!(row["amount"], QueryValue::Int(amount));
}

#[tokio::test]
async fn fts_plain_limit_metadata() {
    let indexed = fixture(true, false, false).await;
    let unindexed = fixture(false, false, false).await;
    let query = Query::from("rows").limit(1);
    let actual = read(&indexed, query.clone(), fts()).await;
    let expected = read(&unindexed, query, fts()).await;
    // Unordered LIMIT need not select the same row; metadata must agree.
    assert_eq!(actual.records.len(), 1);
    assert_eq!(actual.records.len(), expected.records.len());
    assert_eq!(actual.pagination, expected.pagination);
    let page = actual.pagination.unwrap();
    assert!(page.has_next);
    assert_eq!(page.total_count, None);
}

#[tokio::test]
async fn empty_fts_count() {
    // Old shortcut returns no rows instead of the aggregate identity count=0.
    let r = differential(
        Query::from("rows").select([select::count_all("count")]),
        qf::fts("body", "absent", "and"),
        false,
        true,
    )
    .await;
    let v = values(&r);
    assert_eq!(v.len(), 1);
    let QueryValue::Map(map) = &v[0] else {
        panic!("expected map")
    };
    assert_eq!(map["count"], QueryValue::Int(0));
}

#[tokio::test]
async fn corrupt_fts_candidate() {
    // Corruption accounting is an existing invariant; SELECT projection makes
    // this combined differential regression red on the old shortcut.
    let indexed = fixture(true, false, false).await;
    let unindexed = fixture(false, false, false).await;
    for table in [&indexed, &unindexed] {
        let all = read(table, Query::from("rows"), fts()).await;
        assert_eq!(all.records.len(), 3);
        let mut stream = Box::pin(table.list_stream(16));
        let batch = futures::StreamExt::next(&mut stream)
            .await
            .unwrap()
            .unwrap();
        let id: RecordId = batch
            .into_iter()
            .find_map(|(id, cow)| {
                let row = cow.into_inner().unwrap();
                // Choose the same logical row, independent of RID order.
                match row {
                    InnerValue::Map(map)
                        if map.values().any(|v| matches!(v, InnerValue::Int(10))) =>
                    {
                        Some(id)
                    }
                    _ => None,
                }
            })
            .unwrap();
        table
            .data_store()
            .set(
                RecordKey::from_slice(id.as_bytes()),
                Bytes::from_static(b"\xc4"),
            )
            .await
            .unwrap();
        let r = read(table, Query::from("rows").select(["title"]), fts()).await;
        assert_eq!(r.corrupt_records.len(), 1);
        assert_eq!(r.corrupt_records[0].id, id);
        assert_eq!(r.records.len(), 2);
        for v in values(&r) {
            let QueryValue::Map(map) = v else {
                panic!("expected map")
            };
            assert_eq!(map.len(), 1);
        }
    }
}

#[tokio::test]
async fn corrupt_fts_candidate_pagination() {
    // Exercise both a corrupt row inside the old slice and one outside it.
    for position in [0, 2] {
        let indexed = fixture(true, false, false).await;
        let unindexed = fixture(false, false, false).await;
        let full = read(&indexed, Query::from("rows"), fts()).await;
        let full_values = values(&full);
        let QueryValue::Map(row) = &full_values[position] else {
            panic!("expected map")
        };
        let amount = match &row["amount"] {
            QueryValue::Int(amount) => *amount,
            _ => panic!("expected amount"),
        };
        for table in [&indexed, &unindexed] {
            let mut stream = Box::pin(table.list_stream(16));
            let batch = futures::StreamExt::next(&mut stream)
                .await
                .unwrap()
                .unwrap();
            let id = batch
                .into_iter()
                .find_map(|(id, cow)| match cow.into_inner().unwrap() {
                    InnerValue::Map(map)
                        if map
                            .values()
                            .any(|v| matches!(v, InnerValue::Int(n) if *n == amount)) =>
                    {
                        Some(id)
                    }
                    _ => None,
                })
                .unwrap();
            table
                .data_store()
                .set(
                    RecordKey::from_slice(id.as_bytes()),
                    Bytes::from_static(b"\xc4"),
                )
                .await
                .unwrap();
        }
        // count_total forces the unindexed oracle to scan all rows,
        // including corruption outside the page (streaming may stop early).
        for skip in [0, 1] {
            let query = Query::from("rows").offset(skip).limit(1).count_total(true);
            let actual = read(&indexed, query.clone(), fts()).await;
            let expected = read(&unindexed, query, fts()).await;
            assert_eq!(actual.records.len(), 1);
            assert_eq!(actual.records.len(), expected.records.len());
            assert_eq!(actual.pagination, expected.pagination);
            assert_eq!(actual.corrupt_records.len(), 1);
            assert_eq!(expected.corrupt_records.len(), 1);
            let page = actual.pagination.unwrap();
            assert_eq!(page.total_count, Some(2));
            assert_eq!(page.has_next, skip == 0);
        }
    }
}

#[tokio::test]
async fn vector_explicit_order() {
    // Old shortcut preserves ANN rank even when explicit ORDER BY disagrees.
    differential(
        Query::from("rows").order_by_asc("amount").limit(2),
        vector(false),
        true,
        true,
    )
    .await;
}

#[tokio::test]
async fn filtered_vector_count_distinct_and_page() {
    // Old filtered-vector builder projects COUNT without aggregation, leaves
    // duplicate titles, and reports pagination metadata without slicing rows.
    differential(
        Query::from("rows").select([select::count_all("count")]),
        vector(true),
        true,
        true,
    )
    .await;
    differential(
        Query::from("rows").select(["title"]).distinct(),
        vector(true),
        true,
        false,
    )
    .await;
    differential(
        Query::from("rows")
            .order_by_asc("amount")
            .offset(1)
            .limit(1),
        vector(true),
        true,
        true,
    )
    .await;
}

#[tokio::test]
async fn filtered_vector_offset_only_and_versions() {
    // Old filtered-vector path ignores OFFSET and always returns versions=None.
    let table = fixture(true, true, true).await;
    let full = read(&table, Query::from("rows").with_version(), vector(true)).await;
    let page = read(
        &table,
        Query::from("rows").offset(1).with_version(),
        vector(true),
    )
    .await;
    assert_eq!(values(&page), values(&full)[1..]);
    assert_eq!(
        page.versions.as_ref().unwrap(),
        &full.versions.as_ref().unwrap()[1..]
    );
    assert_eq!(page.records.len(), 2);
}

#[tokio::test]
async fn vector_lease_error_is_typed() {
    // Old planner swallows drain errors, then True vector predicates scan all rows.
    for filtered in [false, true] {
        let table = fixture(true, true, false).await;
        let registry = table.index2_registry();
        let drain = registry.reader_gate().begin_drop();
        let interner = table.interner().get().await.unwrap();
        let refs = new_map();
        let ctx = FilterContext::new(interner, &refs);
        let result = table
            .read(&Query::from("rows").build().filter(vector(filtered)), &ctx)
            .await;
        drop(drain);
        assert!(
            matches!(&result, Err(DbError::IndexDrainInProgress(_))),
            "unexpected: {result:?}",
        );
    }
}
