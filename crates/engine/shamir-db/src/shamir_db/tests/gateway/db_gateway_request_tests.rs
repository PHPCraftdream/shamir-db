//! RV-DB-C2DCAC96: the builder-built gateway request must equal the
//! pre-change hand-written literal, field for field, for every key shape.
//! The ORACLE is `legacy_request` below (the original hand-assembled
//! wire request); the ACTOR is the production builder path in
//! `db_gateway.rs` (`request`/`read_query`/`table_ref`/`key_to_filter`).

use super::FacadeDbGateway;
use crate::engine::query::batch::{BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding};
use crate::engine::query::read::{Pagination, ReadQuery, Select, Temporal};
use crate::engine::query::write::{InsertOp, InsertSelect};
use crate::engine::query::{Filter, FilterValue};
use crate::types::common::new_map;
use crate::types::value::QueryValue;

fn legacy_request(alias: &str, id: &str, op: BatchOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        alias.to_string(),
        QueryEntry {
            op,
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Str(id.to_string()),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: false,
        return_only: Some(vec![alias.to_string()]),
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn legacy_read(repo: &str, name: &str, filter: Option<Filter>) -> ReadQuery {
    ReadQuery {
        from: FacadeDbGateway::table_ref(repo, name),
        select: Select::all(),
        r#where: filter,
        group_by: None,
        order_by: None,
        pagination: Pagination::None,
        count_total: false,
        temporal: Temporal::Latest,
        with_version: false,
        explain: false,
    }
}

fn built_read(repo: &str, name: &str, key: &QueryValue) -> ReadQuery {
    let filter = FacadeDbGateway::key_to_filter(key).expect("key converts");
    FacadeDbGateway::read_query(repo, name, filter)
}

fn assert_requests_equal(
    alias: &str,
    id: &str,
    repo: &str,
    table: &str,
    key: &QueryValue,
    expected_filter: Option<Filter>,
) {
    // Production path: key_to_filter + read_query + request.
    let built = FacadeDbGateway::request(alias, id, BatchOp::Read(built_read(repo, table, key)));
    // Oracle: the pre-change hand-written literal, including a hand-written
    // expected filter.
    let old = legacy_request(
        alias,
        id,
        BatchOp::Read(legacy_read(repo, table, expected_filter)),
    );
    assert_eq!(built, old, "repo={repo} table={table} key={key:?}");
}

fn eq_id(value: FilterValue) -> Filter {
    Filter::Eq {
        field: vec!["id".to_string()],
        value,
    }
}

#[test]
fn rv_db_c2dcac96_get_int_key_builder_equals_old_request() {
    assert_requests_equal(
        "r",
        "db_get",
        "main",
        "items",
        &QueryValue::Int(17),
        Some(eq_id(FilterValue::Int(17))),
    );
}

#[test]
fn rv_db_c2dcac96_get_str_key_builder_equals_old_request() {
    assert_requests_equal(
        "r",
        "db_get",
        "main",
        "items",
        &QueryValue::Str("abc".into()),
        Some(eq_id(FilterValue::String("abc".into()))),
    );
}

#[test]
fn rv_db_c2dcac96_get_composite_key_builder_equals_old_request() {
    let mut entries = new_map();
    entries.insert("tenant".to_string(), QueryValue::Str("acme".into()));
    entries.insert("seq".to_string(), QueryValue::Int(7));
    assert_requests_equal(
        "r",
        "db_get",
        "main",
        "items",
        &QueryValue::Map(entries),
        Some(Filter::And {
            filters: vec![
                Filter::Eq {
                    field: vec!["tenant".to_string()],
                    value: FilterValue::String("acme".into()),
                },
                Filter::Eq {
                    field: vec!["seq".to_string()],
                    value: FilterValue::Int(7),
                },
            ],
        }),
    );
}

#[test]
fn rv_db_c2dcac96_insert_builder_equals_old_request() {
    let doc = QueryValue::Str("value".into());
    let built = FacadeDbGateway::request(
        "i",
        "db_insert",
        BatchOp::Insert(
            shamir_query_builder::write::insert("items")
                .row(doc.clone())
                .build(),
        ),
    );
    let old = legacy_request(
        "i",
        "db_insert",
        BatchOp::Insert(InsertOp {
            insert_into: FacadeDbGateway::table_ref("main", "items"),
            values: vec![doc],
            records_idmsgpack: Vec::new(),
            select: None::<InsertSelect>,
        }),
    );
    assert_eq!(built, old);
}

#[test]
fn rv_db_c2dcac96_query_no_filter_builder_equals_old_request() {
    // An empty Map key means "no filter" in the production key_to_filter path.
    assert_requests_equal(
        "q",
        "db_query",
        "archive",
        "items",
        &QueryValue::Map(new_map()),
        None,
    );
}

#[test]
fn rv_db_c2dcac96_query_str_filter_builder_equals_old_request() {
    assert_requests_equal(
        "q",
        "db_query",
        "archive",
        "items",
        &QueryValue::Str("abc".into()),
        Some(eq_id(FilterValue::String("abc".into()))),
    );
}

#[test]
fn rv_db_c2dcac96_query_composite_filter_builder_equals_old_request() {
    let mut entries = new_map();
    entries.insert("a".to_string(), QueryValue::Int(1));
    entries.insert("b".to_string(), QueryValue::Bool(true));
    assert_requests_equal(
        "q",
        "db_query",
        "archive",
        "items",
        &QueryValue::Map(entries),
        Some(Filter::And {
            filters: vec![
                Filter::Eq {
                    field: vec!["a".to_string()],
                    value: FilterValue::Int(1),
                },
                Filter::Eq {
                    field: vec!["b".to_string()],
                    value: FilterValue::Bool(true),
                },
            ],
        }),
    );
}
