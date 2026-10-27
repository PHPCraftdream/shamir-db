//! Owner-local oracles: planner error paths (`UnknownAlias`,
//! `CircularDependency`) and `collect_required_access` over nested batches.

use shamir_collections::{new_map, TMap};
use shamir_types::access::Action;
use shamir_types::types::value::QueryValue;

use crate::batch::planner::BatchPlanner;
use crate::batch::{
    collect_required_access, BatchError, BatchLimits, BatchOp, BatchRequest, QueryEntry,
    ResultEncoding, SubBatchOp,
};
use crate::read::{Pagination, ReadQuery, Select, Temporal};
use crate::TableRef;

fn read_entry(table: &str) -> QueryEntry {
    let q = ReadQuery {
        from: TableRef::new(table),
        r#where: None,
        select: Select::all(),
        order_by: None,
        pagination: Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: Temporal::default(),
        with_version: false,
        explain: false,
    };
    QueryEntry {
        op: BatchOp::Read(q),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

fn batch_request(queries: TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn sub_batch_entry(queries: TMap<String, QueryEntry>) -> QueryEntry {
    QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: batch_request(queries),
            bind: TMap::default(),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

#[test]
fn rv_query_types_b53ec5ce_unknown_alias_in_after_is_rejected() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    let mut a = read_entry("t");
    a.after = vec!["missing".to_string()];
    queries.insert("a".to_string(), a);

    let err = BatchPlanner::plan(&queries, &BatchLimits::default())
        .expect_err("`after` naming a non-existent alias must be rejected");
    assert!(
        matches!(
            &err,
            BatchError::UnknownAlias { alias, referenced_by }
            if alias == "missing" && referenced_by == "a"
        ),
        "expected UnknownAlias{{missing, a}}, got {:?}",
        err
    );
}

#[test]
fn rv_query_types_b53ec5ce_two_alias_cycle_is_circular_dependency() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    let mut a = read_entry("t");
    a.after = vec!["b".to_string()];
    let mut b = read_entry("t");
    b.after = vec!["a".to_string()];
    queries.insert("a".to_string(), a);
    queries.insert("b".to_string(), b);

    let err = BatchPlanner::plan(&queries, &BatchLimits::default())
        .expect_err("a -> b -> a cycle must be rejected");
    assert!(
        matches!(&err, BatchError::CircularDependency { cycle } if cycle.iter().any(|a| a == "a") && cycle.iter().any(|a| a == "b")),
        "expected CircularDependency, got {:?}",
        err
    );
}

#[test]
fn rv_query_types_b53ec5ce_collect_required_access_walks_nested_batches() {
    // outer -> Batch{ inner_batch -> Batch{ deep_read } }, plus a top-level read.
    let mut deepest: TMap<String, QueryEntry> = new_map();
    deepest.insert("deep_read".to_string(), read_entry("secret"));
    let mut middle: TMap<String, QueryEntry> = new_map();
    middle.insert("inner_batch".to_string(), sub_batch_entry(deepest));
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("outer_batch".to_string(), sub_batch_entry(middle));
    queries.insert("top_read".to_string(), read_entry("visible"));

    let required = collect_required_access(&queries, "db");
    let actions: Vec<Action> = required.iter().map(|(a, _)| *a).collect();
    assert_eq!(
        actions,
        vec![Action::Read, Action::Read],
        "both the top-level read and the read nested two Batch levels deep \
         must be collected; a flat walk would yield only one"
    );
}
