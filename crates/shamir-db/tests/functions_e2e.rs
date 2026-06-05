//! End-to-end tests for the built-in function library (`shamir-funclib`)
//! exercised through the public `ShamirDb::execute` wire path — the exact
//! JSON batch format a TCP/WS client sends.
//!
//! Covers the three application points wired into the engine:
//! 1. **Computed values on write** — a field whose value is `{ "$fn": ... }`
//!    is evaluated at insert time and the result is persisted.
//! 2. **Filtering** — a `WHERE` comparison whose value is a `$fn` call is
//!    evaluated per record.
//! 3. **Aggregation** — a `GROUP BY` with a library aggregate
//!    (`SelectItem::AggregateFn`, e.g. `median`).
//!
//! # Migration note
//!
//! Read/write batches are constructed with `shamir_query_builder`. The admin
//! `create_repo` setup batch remains as raw `json!` because `create_repo` is
//! an admin op with no builder coverage.

use serde_json::json;

use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::val::{col, func, lit};
use shamir_query_builder::write::insert;
use shamir_query_builder::{select, Query};

fn to_req(b: &Batch) -> BatchRequest {
    let bytes = b.to_msgpack().expect("msgpack encode");
    rmp_serde::from_slice(&bytes).expect("msgpack decode")
}

/// In-memory ShamirDb with db "testdb", repo "main", table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["users"]),
    );
    shamir.execute("testdb", &to_req(&b)).await.unwrap();
    shamir
}

/// Insert four users, each with a computed `email_norm = strings/lower(email)`.
async fn seed_users(shamir: &ShamirDb) {
    let mut batch = Batch::named("seed");
    batch.id("seed");
    batch.insert(
        "ins",
        insert("users").rows([
            doc! {
                "name" => "alice",
                "city" => "NYC",
                "age" => 30,
                "email" => "A@X.COM",
                "email_norm" => func("strings/lower", [col("email")]),
            },
            doc! {
                "name" => "bob",
                "city" => "LA",
                "age" => 25,
                "email" => "B@X.COM",
                "email_norm" => func("strings/lower", [col("email")]),
            },
            doc! {
                "name" => "carol",
                "city" => "NYC",
                "age" => 35,
                "email" => "C@X.COM",
                "email_norm" => func("strings/lower", [col("email")]),
            },
            doc! {
                "name" => "dave",
                "city" => "LA",
                "age" => 25,
                "email" => "D@X.COM",
                "email_norm" => func("strings/lower", [col("email")]),
            },
        ]),
    );
    let insert_batch = to_req(&batch);
    shamir.execute("testdb", &insert_batch).await.unwrap();
}

/// 1. Computed value on write — the persisted `email_norm` is the lowercased
///    email, proven by an independent Read (not just the insert echo).
#[tokio::test]
async fn e2e_computed_value_persisted_on_insert() {
    let shamir = setup().await;
    seed_users(&shamir).await;

    let mut batch = Batch::new();
    batch.id("r");
    batch.query("all", Query::from("users").where_eq("name", "alice"));
    let read = to_req(&batch);

    let resp = shamir.execute("testdb", &read).await.unwrap();
    let recs = &resp.results["all"].records;
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["email"], json!("A@X.COM"));
    assert_eq!(recs[0]["email_norm"], json!("a@x.com"));
}

/// 2. Filtering — `WHERE name == strings/lower("ALICE")` selects exactly the
///    row whose name equals the computed `"alice"`.
#[tokio::test]
async fn e2e_filter_with_fn_call() {
    let shamir = setup().await;
    seed_users(&shamir).await;

    let mut batch = Batch::new();
    batch.id("r");
    batch.query(
        "match",
        Query::from("users").where_eq("name", func("strings/lower", [lit("ALICE")])),
    );
    let read = to_req(&batch);

    let resp = shamir.execute("testdb", &read).await.unwrap();
    let recs = &resp.results["match"].records;
    assert_eq!(recs.len(), 1, "only alice matches lower(\"ALICE\")");
    assert_eq!(recs[0]["name"], json!("alice"));
}

/// 3. Aggregation — `SELECT city, median(age) GROUP BY city` runs the funclib
///    `median` aggregator per group.
#[tokio::test]
async fn e2e_group_by_library_aggregate() {
    let shamir = setup().await;
    seed_users(&shamir).await;

    let mut batch = Batch::new();
    batch.id("r");
    batch.query(
        "byCity",
        Query::from("users")
            .select([
                select::field("city"),
                select::agg_fn("median", "age", "med_age"),
            ])
            .group_by("city"),
    );
    let read = to_req(&batch);

    let resp = shamir.execute("testdb", &read).await.unwrap();
    let groups = &resp.results["byCity"].records;
    assert_eq!(groups.len(), 2);

    // Groups are emitted in alphabetical key order: LA, then NYC.
    assert_eq!(groups[0]["city"], json!("LA"));
    assert_eq!(groups[0]["med_age"], json!(25)); // [25, 25]
    assert_eq!(groups[1]["city"], json!("NYC"));
    assert_eq!(groups[1]["med_age"], json!(30)); // lower-median of [30, 35]
}

/// 4. Scalar function in the SELECT projection — `SELECT name,
///    strings/upper(name) AS up` evaluates per row.
#[tokio::test]
async fn e2e_select_scalar_function() {
    let shamir = setup().await;
    seed_users(&shamir).await;

    let mut batch = Batch::new();
    batch.id("r");
    batch.query(
        "rows",
        Query::from("users")
            .select([
                select::field("name"),
                select::func("up", "strings/upper", [col("name")]),
            ])
            .where_eq("name", "alice"),
    );
    let read = to_req(&batch);

    let resp = shamir.execute("testdb", &read).await.unwrap();
    let recs = &resp.results["rows"].records;
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["name"], json!("alice"));
    assert_eq!(recs[0]["up"], json!("ALICE"));
}
