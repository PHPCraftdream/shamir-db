//! Tests for [`distinct_repos`]'s recursive walk into `Batch`/`ForEach`
//! bodies (#660).

use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;

use crate::batch::{
    distinct_repos, BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
    SubBatchOp,
};
use crate::filter::FilterValue;
use crate::read::ReadQuery;
use crate::write::InsertOp;
use crate::TableRef;

fn empty_batch() -> BatchRequest {
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn entry(op: BatchOp) -> QueryEntry {
    QueryEntry {
        op,
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

fn insert_into(repo: &str, table: &str) -> BatchOp {
    BatchOp::Insert(InsertOp {
        insert_into: TableRef::with_repo(repo, table),
        values: vec![QueryValue::Map(Default::default())],
        records_idmsgpack: Vec::new(),
        select: None,
    })
}

fn for_each(body: TMap<String, QueryEntry>) -> BatchOp {
    let mut inner = empty_batch();
    inner.queries = body;
    BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: inner,
    })
}

fn sub_batch(body: TMap<String, QueryEntry>) -> BatchOp {
    let mut inner = empty_batch();
    inner.queries = body;
    BatchOp::Batch(SubBatchOp {
        batch: inner,
        bind: TMap::default(),
    })
}

#[test]
fn bare_for_each_body_repo_is_collected() {
    // #660 regression: a batch whose ONLY entry is a ForEach must expose
    // the body's repo (previously returned an empty set).
    let mut body = TMap::default();
    body.insert("ins".to_string(), entry(insert_into("main", "orders")));

    let mut queries = TMap::default();
    queries.insert("loop".to_string(), entry(for_each(body)));

    let repos = distinct_repos(&queries);
    assert_eq!(repos.len(), 1);
    assert!(repos.contains("main"));
}

#[test]
fn bare_sub_batch_body_repo_is_collected() {
    let mut body = TMap::default();
    body.insert("ins".to_string(), entry(insert_into("main", "orders")));

    let mut queries = TMap::default();
    queries.insert("sub".to_string(), entry(sub_batch(body)));

    let repos = distinct_repos(&queries);
    assert_eq!(repos.len(), 1);
    assert!(repos.contains("main"));
}

#[test]
fn nested_body_in_different_repo_yields_both_repos() {
    // Cross-repo guard visibility: a nested body writing to a DIFFERENT
    // repo than the top-level op must surface both repos.
    let mut body = TMap::default();
    body.insert("ins".to_string(), entry(insert_into("hot", "sessions")));

    let mut queries = TMap::default();
    queries.insert(
        "probe".to_string(),
        entry(BatchOp::Read(ReadQuery::with_repo("main", "orders"))),
    );
    queries.insert("loop".to_string(), entry(for_each(body)));

    let repos = distinct_repos(&queries);
    assert_eq!(repos.len(), 2);
    assert!(repos.contains("main"));
    assert!(repos.contains("hot"));
}

#[test]
fn deep_nesting_collects_all_levels() {
    // ForEach → Batch → ForEach: repos from every level are collected.
    let mut innermost = TMap::default();
    innermost.insert("ins".to_string(), entry(insert_into("cold", "archive")));

    let mut mid = TMap::default();
    mid.insert("ins".to_string(), entry(insert_into("hot", "sessions")));
    mid.insert("inner_loop".to_string(), entry(for_each(innermost)));

    let mut outer_body = TMap::default();
    outer_body.insert("ins".to_string(), entry(insert_into("main", "orders")));
    outer_body.insert("mid".to_string(), entry(sub_batch(mid)));

    let mut queries = TMap::default();
    queries.insert("outer_loop".to_string(), entry(for_each(outer_body)));

    let repos = distinct_repos(&queries);
    assert_eq!(repos.len(), 3);
    assert!(repos.contains("main"));
    assert!(repos.contains("hot"));
    assert!(repos.contains("cold"));
}
