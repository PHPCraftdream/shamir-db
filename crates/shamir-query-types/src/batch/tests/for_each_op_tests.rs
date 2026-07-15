//! Serde round-trip + dispatch tests for [`ForEachOp`] (Epic04/B, #653).

use shamir_collections::TMap;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::{BatchLimits, BatchOp, BatchRequest, ForEachOp, ResultEncoding};
use crate::filter::FilterValue;

fn empty_inner_batch() -> BatchRequest {
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

#[test]
fn for_each_serde_roundtrip() {
    let op = BatchOp::ForEach(ForEachOp {
        over: FilterValue::QueryRef {
            alias: "@orders".to_string(),
            path: Some("[].id".to_string()),
        },
        bind_row: "row".to_string(),
        batch: empty_inner_batch(),
    });

    let bytes = rmp_serde::to_vec_named(&op).unwrap();
    let back: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(op, back);
}

#[test]
fn for_each_dispatch_by_for_each_key() {
    let qv: QueryValue = rmp_serde::from_slice(
        &rmp_serde::to_vec_named(&mpack!({
            "over": { "$query": "@orders", "path": "[].id" },
            "bind_row": "row",
            "for_each": { "id": 1_i64, "queries": {} }
        }))
        .unwrap(),
    )
    .unwrap();
    let bytes = rmp_serde::to_vec_named(&qv).unwrap();
    let op: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(op, BatchOp::ForEach(_)));
}

#[test]
fn for_each_is_write_reflects_body() {
    // Empty body → not a write.
    let read_only = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: empty_inner_batch(),
    });
    assert!(!read_only.is_write());

    // A body containing an Insert → write, regardless of iteration count
    // (ADR Decision 5: classification is over the FULL static body, not
    // the runtime iteration count).
    let mut queries = TMap::default();
    queries.insert(
        "ins".to_string(),
        crate::batch::QueryEntry {
            op: BatchOp::Insert(crate::write::InsertOp {
                insert_into: crate::TableRef::new("orders"),
                values: vec![QueryValue::Map(Default::default())],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let mut inner = empty_inner_batch();
    inner.queries = queries;
    let write_body = BatchOp::ForEach(ForEachOp {
        // Zero iterations at runtime — still classified as a write.
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: inner,
    });
    assert!(write_body.is_write());
}

#[test]
fn for_each_table_ref_and_required_access_are_none() {
    let op = BatchOp::ForEach(ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: empty_inner_batch(),
    });
    assert!(op.table_ref().is_none());
    assert!(op.required_access("test_db").is_none());
}
