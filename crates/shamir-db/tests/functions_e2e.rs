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

use serde_json::json;

use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

/// In-memory ShamirDb with db "testdb", repo "main", table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let setup: BatchRequest = serde_json::from_value(json!({
        "id": "setup",
        "queries": {
            "repo": {
                "create_repo": "main",
                "engine": "in_memory",
                "tables": ["users"]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &setup).await.unwrap();
    shamir
}

/// Insert four users, each with a computed `email_norm = strings/lower(email)`.
async fn seed_users(shamir: &ShamirDb) {
    let insert: BatchRequest = serde_json::from_value(json!({
        "id": "seed",
        "queries": {
            "ins": {
                "insert_into": "users",
                "values": [
                    {"name": "alice", "city": "NYC", "age": 30, "email": "A@X.COM",
                     "email_norm": {"$fn": {"name": "strings/lower", "args": [{"$ref": ["email"]}]}}},
                    {"name": "bob",   "city": "LA",  "age": 25, "email": "B@X.COM",
                     "email_norm": {"$fn": {"name": "strings/lower", "args": [{"$ref": ["email"]}]}}},
                    {"name": "carol", "city": "NYC", "age": 35, "email": "C@X.COM",
                     "email_norm": {"$fn": {"name": "strings/lower", "args": [{"$ref": ["email"]}]}}},
                    {"name": "dave",  "city": "LA",  "age": 25, "email": "D@X.COM",
                     "email_norm": {"$fn": {"name": "strings/lower", "args": [{"$ref": ["email"]}]}}}
                ]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &insert).await.unwrap();
}

/// 1. Computed value on write — the persisted `email_norm` is the lowercased
///    email, proven by an independent Read (not just the insert echo).
#[tokio::test]
async fn e2e_computed_value_persisted_on_insert() {
    let shamir = setup().await;
    seed_users(&shamir).await;

    let read: BatchRequest = serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "all": {
                "from": "users",
                "where": {"op": "eq", "field": ["name"], "value": "alice"}
            }
        }
    }))
    .unwrap();
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

    let read: BatchRequest = serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "match": {
                "from": "users",
                "where": {
                    "op": "eq",
                    "field": ["name"],
                    "value": {"$fn": {"name": "strings/lower", "args": ["ALICE"]}}
                }
            }
        }
    }))
    .unwrap();
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

    let read: BatchRequest = serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "byCity": {
                "from": "users",
                "select": {
                    "items": [
                        {"type": "field", "path": ["city"]},
                        {"type": "aggregate_fn", "name": "median",
                         "field": ["age"], "alias": "med_age"}
                    ]
                },
                "group_by": {"fields": [["city"]]}
            }
        }
    }))
    .unwrap();
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

    let read: BatchRequest = serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "rows": {
                "from": "users",
                "select": {
                    "items": [
                        {"type": "field", "path": ["name"]},
                        {"type": "function", "name": "strings/upper",
                         "args": [{"$ref": ["name"]}], "alias": "up"}
                    ]
                },
                "where": {"op": "eq", "field": ["name"], "value": "alice"}
            }
        }
    }))
    .unwrap();
    let resp = shamir.execute("testdb", &read).await.unwrap();
    let recs = &resp.results["rows"].records;
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0]["name"], json!("alice"));
    assert_eq!(recs[0]["up"], json!("ALICE"));
}
