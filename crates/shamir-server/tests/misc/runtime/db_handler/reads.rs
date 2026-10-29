//! Multi-record reads with filter/order/limit and multi-query batches
//! chained through `$query` references.

use shamir_types::mpack;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::upsert;
use shamir_query_builder::Query;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_server::db_handler::{DbResponse, ShamirDbHandler};

use super::common::{decode, encode, execute_built, make_db_with_table, user_session};

// --------------------------------------------------------------------------
// Multi-record read with WHERE + ORDER BY + LIMIT — confirm nothing is
// truncated and stats/pagination flow back to the client.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_with_filter_order_limit_returns_full_payload() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // Seed 5 records via Set ops in a single batch.
    let mut b = Batch::new();
    b.id("seed");
    b.return_only(std::iter::empty::<String>());
    b.try_upsert(
        "s1",
        upsert("items")
            .key(mpack!({"id": "a"}))
            .value(doc! { "id" => "a", "qty" => 3 }),
    )
    .unwrap();
    b.try_upsert(
        "s2",
        upsert("items")
            .key(mpack!({"id": "b"}))
            .value(doc! { "id" => "b", "qty" => 1 }),
    )
    .unwrap();
    b.try_upsert(
        "s3",
        upsert("items")
            .key(mpack!({"id": "c"}))
            .value(doc! { "id" => "c", "qty" => 4 }),
    )
    .unwrap();
    b.try_upsert(
        "s4",
        upsert("items")
            .key(mpack!({"id": "d"}))
            .value(doc! { "id" => "d", "qty" => 1 }),
    )
    .unwrap();
    b.try_upsert(
        "s5",
        upsert("items")
            .key(mpack!({"id": "e"}))
            .value(doc! { "id" => "e", "qty" => 5 }),
    )
    .unwrap();
    let seed = execute_built("prod", b.build());
    let _ = handler
        .handle(
            &session,
            &encode(&seed),
            &ConnectionServices::without_push(0),
        )
        .await
        .unwrap();

    // Query: qty >= 3, ordered by qty DESC, limit 2.
    let mut b = Batch::new();
    b.id("rd");
    b.query(
        "top",
        Query::from("items")
            .where_gte("qty", 3)
            .order_by_desc("qty")
            .limit(2)
            .offset(0),
    );
    let read = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&read),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    };

    let qr = resp.results.get("top").expect("top result present");
    assert_eq!(qr.records.len(), 2, "limit=2 should yield exactly 2 rows");

    // ORDER BY qty DESC over {3,4,5} → top two are 5 then 4.
    assert_eq!(qr.records[0].get_value_i64("qty"), Some(5));
    assert_eq!(qr.records[1].get_value_i64("qty"), Some(4));

    // Stats and pagination metadata flow through.
    assert!(qr.stats.is_some(), "stats should be returned");
    assert!(
        qr.pagination.is_some(),
        "pagination metadata should be returned"
    );

    // Execution plan + timing in the envelope.
    assert!(!resp.execution_plan.is_empty(), "execution_plan present");
    // execution_time_us is a u64; just touch the field to assert presence.
    let _ = resp.execution_time_us;
}

// --------------------------------------------------------------------------
// Multi-query batch with $query reference — independent reads run in
// parallel, dependent reads chain through the planner.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_query_batch_with_query_reference() {
    let shamir = make_db_with_table("prod", "main", "users").await;
    {
        // Add a second table to the same repo via direct API (admin batch
        // ops are exercised separately in `admin_batch_allowed_for_superuser`).
        let db = shamir.get_db("prod").expect("prod db");
        db.create_table("main", "orders").await.expect("add orders");
    }
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // Seed users + orders.
    let mut b = Batch::new();
    b.id("seed");
    b.return_only(std::iter::empty::<String>());
    b.try_upsert(
        "u1",
        upsert("users")
            .key(mpack!({"id": 1}))
            .value(doc! { "id" => 1, "name" => "alice" }),
    )
    .unwrap();
    b.try_upsert(
        "u2",
        upsert("users")
            .key(mpack!({"id": 2}))
            .value(doc! { "id" => 2, "name" => "bob" }),
    )
    .unwrap();
    b.try_upsert(
        "o1",
        upsert("orders")
            .key(mpack!({"id": 100}))
            .value(doc! { "id" => 100, "user_id" => 1, "amt" => 9 }),
    )
    .unwrap();
    b.try_upsert(
        "o2",
        upsert("orders")
            .key(mpack!({"id": 101}))
            .value(doc! { "id" => 101, "user_id" => 1, "amt" => 4 }),
    )
    .unwrap();
    b.try_upsert(
        "o3",
        upsert("orders")
            .key(mpack!({"id": 102}))
            .value(doc! { "id" => 102, "user_id" => 2, "amt" => 7 }),
    )
    .unwrap();
    let seed = execute_built("prod", b.build());
    let _ = handler
        .handle(
            &session,
            &encode(&seed),
            &ConnectionServices::without_push(0),
        )
        .await
        .unwrap();

    // alice's orders: read user, then read orders WHERE user_id = $query
    // reference into the first result.
    let mut b = Batch::new();
    b.id("chained");
    let user_h = b.query("user", Query::from("users").where_eq("name", "alice"));
    b.query(
        "user_orders",
        Query::from("orders").where_eq("user_id", user_h.first().field("id")),
    );
    let chained = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&chained),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    };

    // Both aliases are returned.
    let user = resp.results.get("user").expect("user result");
    assert_eq!(user.records.len(), 1, "alice exists once");
    let orders = resp.results.get("user_orders").expect("orders result");
    assert_eq!(orders.records.len(), 2, "alice has 2 orders");

    // Execution plan must show that `user_orders` is in a later stage than
    // `user` (dependency was honoured by the planner).
    let user_stage = resp
        .execution_plan
        .iter()
        .position(|stage| stage.iter().any(|a| a == "user"))
        .expect("user in plan");
    let orders_stage = resp
        .execution_plan
        .iter()
        .position(|stage| stage.iter().any(|a| a == "user_orders"))
        .expect("user_orders in plan");
    assert!(
        orders_stage > user_stage,
        "user_orders must run after user (got stages {} and {})",
        orders_stage,
        user_stage,
    );
}
