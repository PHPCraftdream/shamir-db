//! Regression tests for `interner_cache_ops::batch_has_refs` (finding 1.4,
//! task #497 — the `@fl` review follow-up: a `$query`/`$param` ref nested
//! inside a `$fn`/`$expr`/`$cond` composite must still be detected, since
//! `execute_with_touch` is a public API accepting an arbitrary caller-built
//! `BatchRequest`, and all three composite constructors are public.

use shamir_collections::TMap;
use shamir_query_types::batch::{BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use shamir_query_types::filter::{Cond, Filter, FilterExpr, FilterExprOp, FilterValue, FnCall};
use shamir_query_types::read::{Select, Temporal};
use shamir_query_types::write::DeleteOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

use crate::interner_cache_ops::batch_has_refs;

fn read_op(field: &str, value: FilterValue) -> BatchOp {
    use shamir_query_types::read::ReadQuery;
    BatchOp::Read(ReadQuery {
        from: TableRef::new("t"),
        select: Select::all(),
        r#where: Some(Filter::Eq {
            field: vec![field.to_string()],
            value,
        }),
        group_by: None,
        order_by: None,
        pagination: Default::default(),
        count_total: false,
        temporal: Temporal::Latest,
        with_version: false,
        explain: false,
    })
}

fn batch_with(op: BatchOp) -> BatchRequest {
    let mut queries = TMap::default();
    queries.insert(
        "q".to_string(),
        QueryEntry {
            op,
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        result_encoding: ResultEncoding::Name,
        interner_epochs: TMap::default(),
    }
}

#[test]
fn flat_query_ref_detected() {
    let op = read_op(
        "name",
        FilterValue::QueryRef {
            alias: "other".into(),
            path: None,
        },
    );
    assert!(batch_has_refs(&batch_with(op)));
}

#[test]
fn no_refs_is_false() {
    let op = read_op("name", FilterValue::String("alice".into()));
    assert!(!batch_has_refs(&batch_with(op)));
}

#[test]
fn query_ref_nested_in_fn_call_is_detected() {
    // { "$fn": { "name": "COALESCE", "args": [{ "$query": "other" }, null] } }
    let op = read_op(
        "name",
        FilterValue::FnCall {
            call: FnCall::complex(
                "COALESCE",
                vec![
                    FilterValue::QueryRef {
                        alias: "other".into(),
                        path: None,
                    },
                    FilterValue::Null,
                ],
            ),
        },
    );
    assert!(
        batch_has_refs(&batch_with(op)),
        "a $query ref nested inside a $fn call must still be detected — \
         a flat (non-recursing) check would silently reintroduce finding 1.4"
    );
}

#[test]
fn param_ref_nested_in_expr_is_detected() {
    // { "$expr": { "op": "add", "args": [{ "$param": "x" }, 1] } }
    let op = read_op(
        "name",
        FilterValue::Expr {
            expr: FilterExpr::new(
                FilterExprOp::Add,
                vec![FilterValue::Param { name: "x".into() }, FilterValue::Int(1)],
            ),
        },
    );
    assert!(
        batch_has_refs(&batch_with(op)),
        "a $param ref nested inside a $expr must still be detected"
    );
}

#[test]
fn query_ref_nested_in_cond_branches_is_detected() {
    // { "$cond": { "if": <filter>, "then": lit, "else": { "$query": "other" } } }
    let op = read_op(
        "name",
        FilterValue::Cond {
            cond: Box::new(Cond::new(
                Filter::Eq {
                    field: vec!["flag".to_string()],
                    value: FilterValue::Bool(true),
                },
                FilterValue::String("yes".into()),
                FilterValue::QueryRef {
                    alias: "other".into(),
                    path: None,
                },
            )),
        },
    );
    assert!(
        batch_has_refs(&batch_with(op)),
        "a $query ref nested in a $cond branch must still be detected"
    );
}

#[test]
fn query_ref_nested_in_cond_condition_is_detected() {
    // { "$cond": { "if": { field: { "$query": "other" } }, "then": ..., "else": ... } }
    let op = read_op(
        "name",
        FilterValue::Cond {
            cond: Box::new(Cond::new(
                Filter::Eq {
                    field: vec!["flag".to_string()],
                    value: FilterValue::QueryRef {
                        alias: "other".into(),
                        path: None,
                    },
                },
                FilterValue::String("yes".into()),
                FilterValue::String("no".into()),
            )),
        },
    );
    assert!(
        batch_has_refs(&batch_with(op)),
        "a $query ref nested in the $cond's own condition filter must still be detected"
    );
}

#[test]
fn delete_where_clause_is_scanned() {
    let op = BatchOp::Delete(DeleteOp {
        delete_from: TableRef::new("t"),
        where_clause: Filter::Eq {
            field: vec!["id".to_string()],
            value: FilterValue::QueryRef {
                alias: "other".into(),
                path: None,
            },
        },
        select: None,
        expected_version: None,
    });
    assert!(batch_has_refs(&batch_with(op)));
}
