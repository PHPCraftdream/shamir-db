// ============================================================================
// Test 6: param_in_insert_values
//
// P3b — canonical case: sub-batch inserts a row where a column value comes
// from a $param bound to the result of an outer query.
//
// Outer:
//   user → read user by name (returns id = 42)
//   sub  → BatchOp::Batch with bind: { uid: $query @user[0].id }
//            inner: insert into orders { user_id: {"$param":"uid"}, note: "order1" }
//   read_back → read orders where user_id == 42
//
// After execution, `read_back` must return exactly one record with user_id=42.
// ============================================================================

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::filter::{Filter, FilterValue, FnCall};
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::sub_batch_basic_tests::{setup, setup_with_scalars};
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::TableRef;

#[tokio::test]
async fn param_in_insert_values() {
    let resolver = setup().await;

    // Seed a user with id field set explicitly.
    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent(
        "seed_user",
        write::insert("users").row(
            doc()
                .set("name", "p3b_alice")
                .unwrap()
                .set("user_id", 42i64)
                .unwrap(),
        ),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Inner batch: insert one order using $param uid as column value.
    let inner_insert_value = mpack!({
        "user_id": { "$param": "uid" },
        "note": "order1"
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_insert_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(60),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Outer batch:
    //   user → read user with name p3b_alice (has user_id=42)
    //   sub  → inner batch with bind: { uid: $query @user[0].user_id }
    //   read_back → read orders where user_id == 42
    let outer_read_q =
        shamir_query_builder::query::Query::from("users").where_eq("name", "p3b_alice");
    let user_entry = QueryEntry {
        op: BatchOp::Read(crate::query::read::ReadQuery::from(outer_read_q)),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    let mut bind = new_map();
    bind.insert(
        "uid".to_string(),
        FilterValue::QueryRef {
            alias: "@user".to_string(),
            path: Some("[0].user_id".to_string()),
        },
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: vec!["user".to_string()],
        when: None,
    };

    let read_back_q = shamir_query_builder::query::Query::from("orders").where_eq("user_id", 42i64);
    let read_back_entry = QueryEntry {
        op: BatchOp::Read(crate::query::read::ReadQuery::from(read_back_q)),
        return_result: true,
        after: vec!["sub".to_string()],
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("user".to_string(), user_entry);
    outer_queries.insert("sub".to_string(), sub_entry);
    outer_queries.insert("read_back".to_string(), read_back_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(6),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let records = &resp.results["read_back"].records;
    assert_eq!(
        records.len(),
        1,
        "read_back must find exactly 1 order with user_id=42; got {:?}",
        records
    );
    assert_eq!(
        records[0].get_value_i64("user_id"),
        Some(42),
        "inserted order must have user_id == 42 (from $param uid)"
    );
    assert_eq!(
        records[0].get_value_str("note"),
        Some("order1"),
        "note field must be preserved verbatim"
    );
}

// ============================================================================
// Test 7: param_in_insert_nested
//
// P3b — $param resolves at depth inside an object nested in an insert value.
// ============================================================================

#[tokio::test]
async fn param_in_insert_nested() {
    let resolver = setup().await;

    // Inner batch: insert a row with a nested object containing $param.
    let inner_value = mpack!({
        "meta": {
            "created_by": { "$param": "actor_id" }
        },
        "label": "nested_test"
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(70),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Bind actor_id = 99.
    let mut bind = new_map();
    bind.insert(
        "actor_id".to_string(),
        // Literal 99 expressed as a FilterValue so the bind resolution
        // path exercises the full pipeline from FilterValue → InnerValue.
        FilterValue::Int(99),
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(7),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The inserted record's nested field must carry the resolved value.
    let sub_val = resp.results["sub"].value.as_ref().unwrap();
    let inserted = &sub_val["ins"]["records"];
    let rows = inserted.as_array().expect("ins.records must be array");
    assert_eq!(rows.len(), 1, "must have inserted 1 row");
    // The stored record is a flat record; nested objects may be serialised as
    // nested msgpack maps. We verify via a direct read.
    drop(resp);

    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders").where_eq("label", "nested_test"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(71),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: read_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let read_resp = execute_batch(&read_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let rows = &read_resp.results["r"].records;
    assert_eq!(rows.len(), 1, "must find the inserted row by label");
    let row0_qv = rows[0].as_value();
    let created_by = row0_qv
        .get("meta")
        .and_then(|m| m.get("created_by"))
        .and_then(|v| v.as_i64());
    assert_eq!(
        created_by,
        Some(99),
        "nested $param must have been substituted to 99"
    );
}

// ============================================================================
// Test 8: param_in_insert_missing_param_errors
//
// P3b — an insert value references a $param that was NOT bound →
// unbound_param error (not a silent miss like in filters).
// ============================================================================

#[tokio::test]
async fn param_in_insert_missing_param_errors() {
    let resolver = setup().await;

    // Inner insert references $param that is NOT in bind.
    let inner_value = mpack!({
        "user_id": { "$param": "ghost_param" }
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(80),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Bind is EMPTY — ghost_param not supplied.
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(8),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let err = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("missing $param in insert value must error");

    assert_eq!(
        err.code(),
        Some("unbound_param"),
        "expected unbound_param, got {:?}",
        err
    );
}

// ============================================================================
// Fix 2 (Finding 8) — user-registered scalar in sub-batch `bind` resolution.
// ============================================================================

/// A sub-batch's `bind` value uses a user-registered scalar (`$fn:
/// my_double(21)`). With the scalar resolver threaded into the bind
/// `FilterContext` (via `.with_scalars(self.resolver.scalar_resolver())` at
/// ~line 350 in `query_runner.rs`), the bind resolves `my_double(21)` to
/// `Int(42)`, injects it as the `$param doubled`, and the inner read's
/// WHERE clause (`score == $param doubled`) matches the seeded row with
/// `score = 42`.
///
/// **Pre-fix behavior** (builtins-only `ScalarResolver`): `my_double` is an
/// unknown function → `resolve_filter_query` returns `None` for the bind
/// value → the bind resolution fails with `BatchError::QueryError` ("cannot
/// resolve filter value"), and the sub-batch never executes. The test
/// would fail at `resp.unwrap()` because the batch would return `Err`.
#[tokio::test]
async fn sub_batch_bind_user_scalar_resolves_correctly() {
    let resolver = setup_with_scalars().await;

    // Seed a user with score=42.
    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent(
        "seed",
        write::insert("users").row(
            doc()
                .set("name", "bind_scalar")
                .unwrap()
                .set("score", 42i64)
                .unwrap(),
        ),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Inner batch: read users where score == $param doubled.
    let inner_where = Filter::Eq {
        field: vec!["score".to_string()],
        value: FilterValue::Param {
            name: "doubled".to_string(),
        },
    };
    let inner_read = shamir_query_types::read::ReadQuery {
        from: crate::query::TableRef::new("users"),
        r#where: Some(inner_where),
        select: shamir_query_types::read::Select::all(),
        order_by: None,
        pagination: shamir_query_types::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: shamir_query_types::read::Temporal::default(),
        with_version: false,
        explain: false,
    };

    let mut inner_queries = new_map();
    inner_queries.insert(
        "match".to_string(),
        QueryEntry {
            op: BatchOp::Read(inner_read),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(30),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Outer: sub-batch with bind: { doubled: $fn my_double(21) }.
    // my_double(21) = 42, so $param doubled = 42 inside the inner batch.
    let mut bind = new_map();
    bind.insert(
        "doubled".to_string(),
        FilterValue::FnCall {
            call: FnCall::complex("my_double", vec![FilterValue::Int(21)]),
        },
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(3),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The sub-batch's inner read must have found the user with score=42.
    let sub_val = resp.results["sub"].value.as_ref().unwrap();
    let match_records = sub_val["match"]["records"]
        .as_array()
        .expect("match.records must be an array");
    assert_eq!(
        match_records.len(),
        1,
        "inner $param-filtered read should return exactly 1 record (score=42, \
         matched by $param doubled = my_double(21) = 42); got {:?}",
        match_records
    );
    assert_eq!(
        match_records[0]["score"], 42i64,
        "the returned record should have score=42"
    );
}
