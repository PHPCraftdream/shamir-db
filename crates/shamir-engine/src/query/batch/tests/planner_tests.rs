//! Tests for BatchPlanner using query builder.
//!
//! All tests use the Batch builder to construct BatchRequests.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::doc;
use shamir_query_builder::{ddl, write};
use shamir_query_types::batch::EdgeKind;
use shamir_types::mpack;

use crate::query::batch::{BatchError, BatchLimits, BatchPlanner, BatchRequest};

#[test]
fn test_plan_empty() {
    let b = Batch::new();
    // Override id to match original test's numeric id
    let mut b = b;
    b.id(1);
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 0);
    assert_eq!(plan.aliases.len(), 0);
}

#[test]
fn test_plan_single_query() {
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.aliases, vec!["users"]);
}

#[test]
fn test_plan_parallel_queries() {
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    b.query("products", Query::from("products"));
    b.query("orders", Query::from("orders"));
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 3);

    assert!(plan.dependencies["users"].is_empty());
    assert!(plan.dependencies["products"].is_empty());
    assert!(plan.dependencies["orders"].is_empty());
}

#[test]
fn test_plan_sequential_dependencies() {
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_eq("user_id", users.all()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);

    assert!(plan.dependencies["users"].is_empty());
    assert!(plan.dependencies["orders"].contains("users"));
}

#[test]
fn test_plan_complex_dependencies() {
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    let products = b.query("products", Query::from("products"));
    let orders = b.query(
        "orders",
        Query::from("orders")
            .where_eq("user_id", users.all())
            .where_eq("product_id", products.all()),
    );
    b.query(
        "stats",
        Query::from("stats").where_eq("order_count", orders.all()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 3);

    assert_eq!(plan.stages[0].len(), 2);
    assert!(plan.stages[0].contains(&"users".to_string()));
    assert!(plan.stages[0].contains(&"products".to_string()));

    assert_eq!(plan.stages[1], vec!["orders"]);
    assert_eq!(plan.stages[2], vec!["stats"]);
}

#[test]
fn test_plan_unknown_alias() {
    let mut b = Batch::new();
    b.id(1);
    b.query(
        "orders",
        Query::from("orders").where_eq(
            "user_id",
            shamir_query_builder::val::qref_all("nonexistent"),
        ),
    );
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(err, crate::query::batch::BatchError::UnknownAlias { alias, .. } if alias == "nonexistent")
    );
}

#[test]
fn test_plan_too_many_queries() {
    let mut b = Batch::new();
    b.id(1);
    for i in 0..60 {
        b.query(format!("q{}", i), Query::from("table"));
    }
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::TooManyQueries { count: 60, max: 50 }
    ));
}

#[test]
fn test_plan_custom_limits() {
    let mut b = Batch::new();
    b.id(1);
    b.query("a", Query::from("t"));
    b.query("b", Query::from("t"));
    b.query("c", Query::from("t"));
    b.query("d", Query::from("t"));
    b.limits(BatchLimits {
        max_queries: 3,
        max_dependency_depth: 10,
        max_execution_time_secs: 30,
        max_result_size: 10_000_000,
        max_nesting_depth: 4,
        max_iterations: 1000,
    });
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &request.limits).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::TooManyQueries { count: 4, max: 3 }
    ));
}

#[test]
fn test_plan_circular_dependency() {
    let mut b = Batch::new();
    b.id(1);
    // Circular: a -> c -> b -> a. Must use raw qref since Handle can't
    // express forward references to not-yet-registered aliases.
    b.query(
        "a",
        Query::from("a").where_eq("x", shamir_query_builder::val::qref_all("c")),
    );
    b.query(
        "b",
        Query::from("b").where_eq("x", shamir_query_builder::val::qref_all("a")),
    );
    b.query(
        "c",
        Query::from("c").where_eq("x", shamir_query_builder::val::qref_all("b")),
    );
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::CircularDependency { .. }
    ));
}

#[test]
fn test_plan_self_dependency() {
    let mut b = Batch::new();
    b.id(1);
    b.query(
        "self_ref",
        Query::from("t").where_eq("x", shamir_query_builder::val::qref_all("self_ref")),
    );
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::CircularDependency { .. }
    ));
}

#[test]
fn test_plan_dependency_depth() {
    let mut b = Batch::new();
    b.id(1);
    b.query("q0", Query::from("t"));
    for i in 1..15 {
        b.query(
            format!("q{}", i),
            Query::from("t").where_eq(
                "x",
                shamir_query_builder::val::qref_all(format!("q{}", i - 1)),
            ),
        );
    }
    b.limits(BatchLimits {
        max_queries: 50,
        max_dependency_depth: 10,
        max_execution_time_secs: 30,
        max_result_size: 10_000_000,
        max_nesting_depth: 4,
        max_iterations: 1000,
    });
    let request = b.build();
    let err = BatchPlanner::plan(&request.queries, &request.limits).unwrap_err();
    assert!(matches!(
        err,
        crate::query::batch::BatchError::TooDeep { depth: 14, max: 10 }
    ));
}

#[test]
fn test_plan_mixed_in_filter() {
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_in("user_id", [users.all(), 42.into()]),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);
}

#[test]
fn test_plan_or_filter() {
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    let admins = b.query("admins", Query::from("admins"));
    b.query(
        "results",
        Query::from("results")
            .where_eq("user_id", users.all())
            .or_where_eq("admin_id", admins.all()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0].len(), 2);
    assert_eq!(plan.stages[1], vec!["results"]);
}

#[test]
fn test_plan_diamond_dependency() {
    let mut b = Batch::new();
    b.id(1);
    let a = b.query("a", Query::from("t"));
    let b_h = b.query("b", Query::from("t").where_eq("x", a.all()));
    let c_h = b.query("c", Query::from("t").where_eq("x", a.all()));
    b.query(
        "d",
        Query::from("t")
            .where_eq("x", b_h.all())
            .where_eq("y", c_h.all()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 3);
    assert_eq!(plan.stages[0], vec!["a"]);
    assert_eq!(plan.stages[1].len(), 2);
    assert_eq!(plan.stages[2], vec!["d"]);
}

#[test]
fn test_plan_with_query_path() {
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_eq("user_id", users.first().field("id")),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["orders"]);
}

#[test]
fn test_plan_return_flags() {
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    b.query_silent("internal", Query::from("internal"));
    b.return_only(["users"]);
    let request = b.build();
    assert_eq!(request.queries.len(), 2);
    assert!(request.queries.get("users").unwrap().return_result);
    assert!(!request.queries.get("internal").unwrap().return_result);
    assert!(!request.return_all);
    assert_eq!(request.return_only, Some(vec!["users".to_string()]));
}

#[test]
fn test_plan_transactional_batch() {
    // NOTE: the original test used inline "$query": "users[0].id" syntax
    // (a single string with path embedded). The builder produces the split
    // form {"$query": "@users", "path": "[0].id"} which is semantically
    // identical for the planner. Using the same approach here.
    let mut b = Batch::new();
    b.id(1);
    b.name("user_order_transaction");
    b.transactional();
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_eq("user_id", users.first().field("id")),
    );
    let request = b.build();
    assert!(request.transactional);
    assert_eq!(request.name, Some("user_order_transaction".to_string()));
}

#[test]
fn test_plan_not_filter() {
    // NOTE: original test used inline "$query": "active_users[0].id" syntax.
    // Builder produces split form with @-prefix — planner handles both.
    let mut b = Batch::new();
    b.id(1);
    let active = b.query("active_users", Query::from("users"));
    b.query(
        "inactive_users",
        Query::from("users").where_(shamir_query_builder::filter::not(
            shamir_query_builder::filter::eq("id", active.first().field("id")),
        )),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["active_users"]);
    assert_eq!(plan.stages[1], vec!["inactive_users"]);
}

// ============================================================================
// WRITE OPERATIONS TESTS
// ============================================================================

#[test]
fn test_plan_insert_operation() {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert_user",
        write::insert("users").row(doc().set("name", "Alice").set("email", "alice@example.com")),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0], vec!["insert_user"]);
    assert!(plan.dependencies["insert_user"].is_empty());
}

#[test]
fn test_plan_update_operation() {
    // NOTE: original test used inline "$query": "users[0].id". Builder
    // produces split form — semantically identical for planner dep extraction.
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.update(
        "update_orders",
        write::update("orders")
            .where_(shamir_query_builder::filter::eq(
                "user_id",
                users.first().field("id"),
            ))
            .set(doc().set("status", "processed")),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["update_orders"]);
    assert!(plan.dependencies["update_orders"].contains("users"));
}

#[test]
fn test_plan_set_operation() {
    // NOTE: original used inline "$query": "users[0].id" in the key object.
    // The builder uses doc().set("id", handle.first().field("id")) which
    // serializes to the same $query ref structure.
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.upsert(
        "set_user",
        write::upsert("users")
            .key(doc().set("id", users.first().field("id")).build())
            .value(doc().set("status", "updated").build()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["users"]);
    assert_eq!(plan.stages[1], vec!["set_user"]);
    assert!(plan.dependencies["set_user"].contains("users"));
}

#[test]
fn test_plan_delete_operation() {
    // NOTE: original used inline "$query": "inactive_users[].id" in In values.
    // Builder uses handle.column("id") which produces "[].id" path.
    let mut b = Batch::new();
    b.id(1);
    let inactive = b.query(
        "inactive_users",
        Query::from("users").where_eq("status", "inactive"),
    );
    b.delete(
        "delete_orders",
        write::delete("orders").where_(shamir_query_builder::filter::in_(
            "user_id",
            [inactive.column("id")],
        )),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["inactive_users"]);
    assert_eq!(plan.stages[1], vec!["delete_orders"]);
    assert!(plan.dependencies["delete_orders"].contains("inactive_users"));
}

#[test]
fn test_plan_set_with_value_reference() {
    // NOTE: original used inline "$query": "source[0].name" in value object.
    let mut b = Batch::new();
    b.id(1);
    let source = b.query("source", Query::from("source_table"));
    b.upsert(
        "set_target",
        write::upsert("target_table")
            .key(doc().set("id", 1).build())
            .value(
                doc()
                    .set("name", source.first().field("name"))
                    .set("status", "copied")
                    .build(),
            ),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert!(plan.dependencies["set_target"].contains("source"));
}

#[test]
fn test_plan_insert_with_value_reference() {
    // NOTE: original used inline "$query": "user[0].id".
    let mut b = Batch::new();
    b.id(1);
    let user = b.query("user", Query::from("users").where_eq("id", 1));
    b.insert(
        "insert_order",
        write::insert("orders").row(
            doc()
                .set("user_id", user.first().field("id"))
                .set("product", "Widget"),
        ),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["user"]);
    assert_eq!(plan.stages[1], vec!["insert_order"]);
}

#[test]
fn test_plan_mixed_read_and_write() {
    let mut b = Batch::new();
    b.id(1);
    b.query("users", Query::from("users"));
    b.query("products", Query::from("products"));
    b.insert(
        "insert_order",
        write::insert("orders").row(doc().set("user_id", 1).set("product_id", 1)),
    );
    b.update(
        "update_inventory",
        write::update("inventory")
            .where_(shamir_query_builder::filter::eq("product_id", 1))
            .set(doc().set("quantity", 0)),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 4);
}

#[test]
fn test_plan_update_without_filter() {
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "update_all",
        write::update("users").set(doc().set("status", "active")),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 1);
    assert!(plan.dependencies["update_all"].is_empty());
}

#[test]
fn test_plan_write_operations_serialization() {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "insert",
        write::insert("users").row(doc().set("name", "Test")),
    );
    b.update(
        "update",
        write::update("users")
            .where_(shamir_query_builder::filter::eq("name", "Test"))
            .set(doc().set("name", "Updated")),
    );
    b.upsert(
        "set",
        write::upsert("users")
            .key(doc().set("id", 1).build())
            .value(doc().set("name", "Set").build()),
    );
    b.delete(
        "delete",
        write::delete("users").where_(shamir_query_builder::filter::eq("id", 999)),
    );
    let request = b.build();
    assert_eq!(request.queries.len(), 4);

    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
    assert_eq!(plan.stages.len(), 1);
    assert_eq!(plan.stages[0].len(), 4);
}

// ============================================================================
// EXPLICIT `after` ORDERING TESTS
// ============================================================================

#[test]
fn test_after_orders_into_later_stage() {
    // B has `after: ["a"]` with no $query refs — should land in a later stage.
    let mut b = Batch::new();
    b.id(1);
    let a = b.create_table("a", ddl::create_table("users").repo("main"));
    let rows = b.insert("b", write::insert("users").row(doc().set("name", "Alice")));
    b.after(&rows, &a);
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["a"]);
    assert_eq!(plan.stages[1], vec!["b"]);
    assert!(plan.dependencies["b"].contains("a"));
}

#[test]
fn test_after_unknown_alias_error() {
    // mpack! + rmp_serde by design: this is a planner *validation* test. The
    // builder's `after()` takes a Handle, so it cannot construct an `after`
    // referencing an alias that isn't registered — which is exactly the invalid
    // input this test must feed the planner. The raw wire form (msgpack) is
    // the thing under test, not a builder gap.
    let raw = mpack!({
        "id": 1,
        "queries": {
            "a": {
                "insert_into": "users",
                "values": [{"name": "Alice"}],
                "after": ["nonexistent"]
            }
        }
    });
    let bytes = rmp_serde::to_vec_named(&raw).unwrap();
    let request: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(
            &err,
            crate::query::batch::BatchError::UnknownAlias { alias, .. }
            if alias == "nonexistent"
        ),
        "expected UnknownAlias for 'nonexistent', got: {:?}",
        err
    );
}

#[test]
fn test_after_circular_dependency() {
    // mpack! + rmp_serde by design: a mutual `after` (circular) needs forward
    // references the Handle-based `after()` cannot express — and that
    // impossibility is the point. This validation test feeds the planner the
    // invalid wire form (msgpack) directly.
    let raw = mpack!({
        "id": 1,
        "queries": {
            "a": {
                "create_table": "t1",
                "repo": "main",
                "after": ["b"]
            },
            "b": {
                "create_table": "t2",
                "repo": "main",
                "after": ["a"]
            }
        }
    });
    let bytes = rmp_serde::to_vec_named(&raw).unwrap();
    let request: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(
            err,
            crate::query::batch::BatchError::CircularDependency { .. }
        ),
        "expected CircularDependency, got: {:?}",
        err
    );
}

#[test]
fn test_after_with_at_prefix_normalizes() {
    // `after: ["@a"]` should normalize to alias "a".
    // The builder's after() always uses bare aliases, so to test the
    // @-prefix normalization in the planner we use the msgpack wire form directly.
    let raw = mpack!({
        "id": 1,
        "queries": {
            "a": {
                "create_table": "users",
                "repo": "main"
            },
            "b": {
                "insert_into": "users",
                "values": [{"name": "Bob"}],
                "after": ["@a"]
            }
        }
    });
    let bytes = rmp_serde::to_vec_named(&raw).unwrap();
    let request: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    assert_eq!(plan.stages.len(), 2);
    assert_eq!(plan.stages[0], vec!["a"]);
    assert_eq!(plan.stages[1], vec!["b"]);
    assert!(plan.dependencies["b"].contains("a"));
}

// ============================================================================
// EDGE PROVENANCE TESTS (task #628 — Epic01/A)
// ============================================================================

#[test]
fn edge_provenance_pure_after_is_explicit() {
    // b `after` a, with no `$query` ref between them → Explicit only.
    let mut b = Batch::new();
    b.id(1);
    let a = b.create_table("a", ddl::create_table("users").repo("main"));
    let rows = b.insert("b", write::insert("users").row(doc().set("name", "Alice")));
    b.after(&rows, &a);
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    let provenance = plan
        .edge_provenance
        .get("b")
        .expect("b must have provenance entry");
    assert_eq!(provenance.get("a"), Some(&EdgeKind::Explicit));
}

#[test]
fn edge_provenance_pure_query_ref_is_dataflow() {
    // orders depends on users purely via $query (where_eq) — no `after`.
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_eq("user_id", users.all()),
    );
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have provenance entry");
    assert_eq!(provenance.get("users"), Some(&EdgeKind::DataFlow));
}

#[test]
fn edge_provenance_after_and_query_ref_on_same_alias_is_both() {
    // orders both `after`s users AND references it via $query — dedup with
    // both flags preserved (EdgeKind::Both), not an error.
    let mut b = Batch::new();
    b.id(1);
    let users = b.query("users", Query::from("users"));
    let orders = b.query(
        "orders",
        Query::from("orders").where_eq("user_id", users.all()),
    );
    b.after(&orders, &users);
    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have provenance entry");
    assert_eq!(provenance.get("users"), Some(&EdgeKind::Both));
}

#[test]
fn after_with_bracket_path_tail_is_rejected() {
    // `after` naming a value-path tail (e.g. "users[0].id") is a documented
    // developer mistake — `after` is alias-only ordering and never resolves
    // a value path the way `$query` does. Planning must fail fast instead of
    // silently stripping to the base alias.
    let raw = mpack!({
        "id": 1,
        "queries": {
            "users": {
                "from": "users"
            },
            "b": {
                "insert_into": "users",
                "values": [{"name": "Bob"}],
                "after": ["users[0].id"]
            }
        }
    });
    let bytes = rmp_serde::to_vec_named(&raw).unwrap();
    let request: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(
            &err,
            BatchError::AfterPathIgnored { alias, raw }
            if alias == "b" && raw == "users[0].id"
        ),
        "expected AfterPathIgnored for 'users[0].id', got: {:?}",
        err
    );
}

#[test]
fn after_with_dot_path_tail_is_rejected() {
    let raw = mpack!({
        "id": 1,
        "queries": {
            "users": {
                "from": "users"
            },
            "b": {
                "insert_into": "users",
                "values": [{"name": "Bob"}],
                "after": ["users.id"]
            }
        }
    });
    let bytes = rmp_serde::to_vec_named(&raw).unwrap();
    let request: BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    let err = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err();
    assert!(
        matches!(&err, BatchError::AfterPathIgnored { alias, raw } if alias == "b" && raw == "users.id"),
        "expected AfterPathIgnored for 'users.id', got: {:?}",
        err
    );
}

// ============================================================================
// Multi-hop mixed chain (task #630 — Epic01/C gap #1)
// ============================================================================
//
// A -> B by pure `$query` (DataFlow), B -> C by pure `after` (Explicit),
// C -> D by BOTH `after` and `$query` on the same alias (Both). Existing
// coverage (edge_provenance_* tests above) only exercises each edge kind in
// isolation between a single pair of aliases; this test walks a >=3-hop
// chain mixing all three kinds end to end, and asserts both the stage
// ordering and the provenance recorded at each hop.

#[test]
fn multi_hop_chain_mixes_dataflow_explicit_and_both_edges() {
    let mut b = Batch::new();
    b.id(1);

    // A -> B: pure $query (DataFlow).
    let a = b.query("a", Query::from("t"));
    let b_h = b.query("b", Query::from("t").where_eq("x", a.all()));

    // B -> C: pure `after` (Explicit) — C has no $query ref to b at all.
    let c = b.query("c", Query::from("t"));
    b.after(&c, &b_h);

    // C -> D: BOTH `after` AND `$query` on the same alias (Both).
    let d = b.query("d", Query::from("t").where_eq("y", c.all()));
    b.after(&d, &c);

    let request = b.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();

    // Strict 4-stage chain: a < b < c < d.
    assert_eq!(
        plan.stages.len(),
        4,
        "expected 4 sequential stages: {plan:?}"
    );
    assert_eq!(plan.stages[0], vec!["a"]);
    assert_eq!(plan.stages[1], vec!["b"]);
    assert_eq!(plan.stages[2], vec!["c"]);
    assert_eq!(plan.stages[3], vec!["d"]);

    // Provenance per hop.
    assert_eq!(
        plan.edge_provenance.get("b").and_then(|p| p.get("a")),
        Some(&EdgeKind::DataFlow),
        "a->b must be DataFlow-only"
    );
    assert_eq!(
        plan.edge_provenance.get("c").and_then(|p| p.get("b")),
        Some(&EdgeKind::Explicit),
        "b->c must be Explicit-only"
    );
    assert_eq!(
        plan.edge_provenance.get("d").and_then(|p| p.get("c")),
        Some(&EdgeKind::Both),
        "c->d must be Both"
    );
}
