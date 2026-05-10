//! Integration tests for the `RequestHandler` ↔ `ShamirDb` bridge.
//!
//! These tests prove that the wire-side `Execute { db, batch }` path
//! preserves the full feature set of the underlying `BatchRequest`/
//! `BatchResponse` API: multi-record reads, projections, ordering,
//! pagination, multi-query batches with `$query` references, admin DDL,
//! and the superuser permission gate.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::json;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::db::engine::table::TableConfig;
use shamir_db::db::query::batch::BatchRequest;
use shamir_db::db::ShamirDb;

use shamir_server::db_handler::{AdminGlue, DbRequest, DbResponse, ShamirDbHandler};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_server::user_directory::RedbUserDirectory;
use tempfile::TempDir;

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

fn make_session(roles: Vec<String>) -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(roles),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

fn user_session() -> Session {
    make_session(vec!["read_write".into()])
}

fn root_session() -> Session {
    make_session(vec!["superuser".into()])
}

async fn make_db_with_table(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory())
        .add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

/// Build a `DbRequest::Execute` from a JSON batch body, defaulting the
/// query-language version to `CURRENT_QUERY_LANG_VERSION`. Keeps tests
/// terse — each test reads like the JSON a real client would send.
fn execute(db: &str, body: serde_json::Value) -> DbRequest {
    execute_with_version(db, shamir_server::version::CURRENT_QUERY_LANG_VERSION, body)
}

/// Same as [`execute`] but with an explicit `query_version` for the
/// version-dispatch tests.
fn execute_with_version(db: &str, query_version: u32, body: serde_json::Value) -> DbRequest {
    let batch: BatchRequest = serde_json::from_value(body).expect("parse batch");
    DbRequest::Execute {
        query_version,
        db: db.to_string(),
        batch,
    }
}

// --------------------------------------------------------------------------
// Health + protocol-level errors
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_returns_pong() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = user_session();

    let res = handler.handle(&session, &encode(&DbRequest::Ping)).unwrap();
    assert!(matches!(decode(&res), DbResponse::Pong));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_msgpack_returns_protocol_err() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = user_session();

    let garbage: &[u8] = &[0xff, 0x00, 0x10, 0x42, 0x99, 0x01];
    let err = handler.handle(&session, garbage).unwrap_err();
    assert!(err.starts_with("invalid_request:"), "got {:?}", err);
}

// --------------------------------------------------------------------------
// Unknown DB → wire-level error with `kind = "unknown_db"`
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_db_returns_typed_error() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = user_session();

    let req = execute(
        "nope",
        json!({
            "id": 1,
            "queries": { "ping": { "from": "users" } }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "unknown_db");
            assert!(message.contains("not found"), "got {:?}", message);
        }
        other => panic!("expected unknown_db Error, got {:?}", other),
    }
}

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
    let seed = execute(
        "prod",
        json!({
            "id": "seed",
            "queries": {
                "s1": { "set": "items", "key": {"id": "a"}, "value": {"id":"a","qty":3} },
                "s2": { "set": "items", "key": {"id": "b"}, "value": {"id":"b","qty":1} },
                "s3": { "set": "items", "key": {"id": "c"}, "value": {"id":"c","qty":4} },
                "s4": { "set": "items", "key": {"id": "d"}, "value": {"id":"d","qty":1} },
                "s5": { "set": "items", "key": {"id": "e"}, "value": {"id":"e","qty":5} }
            },
            "return_all": false
        }),
    );
    let _ = handler.handle(&session, &encode(&seed)).unwrap();

    // Query: qty >= 3, ordered by qty DESC, limit 2.
    let read = execute(
        "prod",
        json!({
            "id": "rd",
            "queries": {
                "top": {
                    "from": "items",
                    "where": { "op": "gte", "field": ["qty"], "value": 3 },
                    "order_by": { "items": [{ "field": ["qty"], "direction": "desc" }] },
                    "pagination": { "mode": "LimitOffset", "limit": 2, "offset": 0 }
                }
            }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&read)).unwrap());
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch, got {:?}", other),
    };

    let qr = resp.results.get("top").expect("top result present");
    assert_eq!(qr.records.len(), 2, "limit=2 should yield exactly 2 rows");

    // ORDER BY qty DESC over {3,4,5} → top two are 5 then 4.
    assert_eq!(qr.records[0].get("qty").and_then(|v| v.as_i64()), Some(5));
    assert_eq!(qr.records[1].get("qty").and_then(|v| v.as_i64()), Some(4));

    // Stats and pagination metadata flow through.
    assert!(qr.stats.is_some(), "stats should be returned");
    assert!(qr.pagination.is_some(), "pagination metadata should be returned");

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
        db.create_table("main", "orders").expect("add orders table");
    }
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // Seed users + orders.
    let seed = execute(
        "prod",
        json!({
            "id": "seed",
            "queries": {
                "u1": { "set": "users", "key": {"id": 1}, "value": {"id": 1, "name": "alice"} },
                "u2": { "set": "users", "key": {"id": 2}, "value": {"id": 2, "name": "bob"} },
                "o1": { "set": "orders", "key": {"id": 100}, "value": {"id": 100, "user_id": 1, "amt": 9} },
                "o2": { "set": "orders", "key": {"id": 101}, "value": {"id": 101, "user_id": 1, "amt": 4} },
                "o3": { "set": "orders", "key": {"id": 102}, "value": {"id": 102, "user_id": 2, "amt": 7} }
            },
            "return_all": false
        }),
    );
    let _ = handler.handle(&session, &encode(&seed)).unwrap();

    // alice's orders: read user, then read orders WHERE user_id = $query
    // reference into the first result.
    let chained = execute(
        "prod",
        json!({
            "id": "chained",
            "queries": {
                "user": {
                    "from": "users",
                    "where": { "op": "eq", "field": ["name"], "value": "alice" }
                },
                "user_orders": {
                    "from": "orders",
                    "where": {
                        "op": "eq",
                        "field": ["user_id"],
                        "value": { "$query": "user", "path": "[0].id" }
                    }
                }
            }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&chained)).unwrap());
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

// --------------------------------------------------------------------------
// Admin DDL through the wire — superuser allowed.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_batch_allowed_for_superuser() {
    // Start with the DB present but no `inventory` table yet; create it via
    // a wire-side admin batch.
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db("prod").await;
    let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("prod", cfg).await.expect("add repo");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let admin = execute(
        "prod",
        json!({
            "id": "ddl",
            "queries": {
                "mk": { "create_table": "inventory", "repo": "main" }
            }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&admin)).unwrap());
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch from create_table, got {:?}", other),
    };
    assert!(resp.results.contains_key("mk"), "mk result present");

    // Verify table is usable: insert + read back.
    let rw = execute(
        "prod",
        json!({
            "id": "rw",
            "queries": {
                "ins": { "set": "inventory", "key": {"sku": "X1"}, "value": {"sku":"X1","stock":42} },
                "rd":  { "from": "inventory" }
            }
        }),
    );
    let res2 = decode(&handler.handle(&session, &encode(&rw)).unwrap());
    let resp2 = match res2 {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch on read, got {:?}", other),
    };
    let rows = &resp2.results.get("rd").expect("rd alias").records;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("sku").and_then(|v| v.as_str()), Some("X1"));
    assert_eq!(rows[0].get("stock").and_then(|v| v.as_i64()), Some(42));
}

// --------------------------------------------------------------------------
// Admin DDL through the wire — non-superuser denied with typed error.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_batch_denied_for_non_superuser() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session(); // not superuser

    let admin = execute(
        "prod",
        json!({
            "id": "ddl",
            "queries": {
                "drop": { "drop_table": "items", "repo": "main" }
            }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&admin)).unwrap());
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "permission_denied");
            assert!(message.contains("superuser"), "got {:?}", message);
        }
        other => panic!("expected permission_denied, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Echo: BatchResponse.id matches BatchRequest.id (request correlation).
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_response_echoes_request_id() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    let req = execute(
        "prod",
        json!({
            "id": "client-correlation-token-42",
            "queries": { "all": { "from": "items" } }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::Batch { response } => {
            assert_eq!(
                response.id,
                json!("client-correlation-token-42"),
                "BatchResponse.id should echo BatchRequest.id verbatim"
            );
        }
        other => panic!("expected Batch, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Query-language version dispatch — unsupported version → typed error.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_query_version_rejected_before_db_work() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // 99 is not in `SUPPORTED_QUERY_LANG_VERSIONS`. Expect a typed error
    // BEFORE the batch hits the DB layer.
    let req = execute_with_version(
        "prod",
        99,
        json!({
            "id": "v",
            "queries": { "all": { "from": "items" } }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "unsupported_query_version");
            assert!(message.contains("99"), "got {:?}", message);
        }
        other => panic!("expected unsupported_query_version, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_query_version_accepted() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    let req = execute_with_version(
        "prod",
        shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        json!({
            "id": "v",
            "queries": { "all": { "from": "items" } }
        }),
    );
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    assert!(matches!(res, DbResponse::Batch { .. }),
        "current version must be accepted; got {:?}", res);
}

// --------------------------------------------------------------------------
// CreateScramUser — wire-side SCRAM user creation.
// --------------------------------------------------------------------------

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_denied_without_admin_glue() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db)); // no admin glue
    let session = root_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec!["user".into()],
    };
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::Error { code, .. } => assert_eq!(code, "not_supported"),
        other => panic!("expected not_supported, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_denied_for_non_superuser() {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(RedbUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue { user_dir: user_dir.clone(), kdf: fast_kdf(), tables_registry: None },
    );
    let session = user_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec![],
    };
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::Error { code, .. } => assert_eq!(code, "permission_denied"),
        other => panic!("expected permission_denied, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_scram_user_success_then_duplicate() {
    let tmp = TempDir::new().unwrap();
    let user_dir = Arc::new(RedbUserDirectory::open(tmp.path().join("u.redb")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::with_admin(
        Arc::new(db),
        AdminGlue { user_dir: user_dir.clone(), kdf: fast_kdf(), tables_registry: None },
    );
    let session = root_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec!["read_write".into()],
    };
    let res = decode(&handler.handle(&session, &encode(&req)).unwrap());
    match res {
        DbResponse::UserCreated { name, user_id } => {
            assert_eq!(name, "bob");
            assert_eq!(user_id.len(), 16, "user_id is a stable 16-byte handle");
        }
        other => panic!("expected UserCreated, got {:?}", other),
    }

    use shamir_connect::server::admin::UserDirectory;
    assert!(user_dir.lookup_by_name("bob").is_some(), "persisted in directory");
    let roles = user_dir.lookup_roles("bob").unwrap_or_default();
    assert!(roles.iter().any(|r| r == "read_write"), "roles attached");

    // Second insert with same name -> typed user_exists error.
    let req2 = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "another password".into(),
        roles: vec![],
    };
    let res2 = decode(&handler.handle(&session, &encode(&req2)).unwrap());
    match res2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "user_exists"),
        other => panic!("expected user_exists, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Sanity: `IndexMap` is in the import set so that future tests can build
// `TMap` directly without going through JSON if they need to.
// --------------------------------------------------------------------------
#[allow(dead_code)]
fn _indexmap_in_scope() -> IndexMap<String, u32> {
    IndexMap::new()
}
