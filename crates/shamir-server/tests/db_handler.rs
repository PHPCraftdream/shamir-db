//! Integration tests for the `RequestHandler` ↔ `ShamirDb` bridge.
//!
//! These tests prove that the wire-side `Execute { db, batch }` path
//! preserves the full feature set of the underlying `BatchRequest`/
//! `BatchResponse` API: multi-record reads, projections, ordering,
//! pagination, multi-query batches with `$query` references, admin DDL,
//! and the superuser permission gate.
//!
//! # Migration note
//!
//! All batch requests (DML + DDL) are constructed with
//! `shamir_query_builder` and round-tripped through MessagePack.

use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::json;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_connect::common::kdf_params::KdfParams;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::{insert, upsert};
use shamir_query_builder::Query;
use shamir_server::db_handler::{
    AdminGlue, DbRequest, DbResponse, QueryLimitsCap, ShamirDbHandler, TxLimitsCap,
};
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
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

/// Build a `DbRequest::Execute` from a pre-built [`BatchRequest`].
fn execute_built(db: &str, batch: BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: db.to_string(),
        batch,
    }
}

/// Same as above but with an explicit `query_version`.
fn execute_built_with_version(db: &str, query_version: u32, batch: BatchRequest) -> DbRequest {
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

    let res = handler
        .handle(
            &session,
            &encode(&DbRequest::Ping),
            &ConnectionServices::without_push(0),
        )
        .await
        .unwrap();
    assert!(matches!(decode(&res), DbResponse::Pong));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_msgpack_returns_protocol_err() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = user_session();

    let garbage: &[u8] = &[0xff, 0x00, 0x10, 0x42, 0x99, 0x01];
    let err = handler
        .handle(&session, garbage, &ConnectionServices::without_push(0))
        .await
        .unwrap_err();
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

    let mut b = Batch::new();
    b.id(1);
    b.query("ping", Query::from("users"));
    let req = execute_built("nope", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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
    let mut b = Batch::new();
    b.id("seed");
    b.return_only(std::iter::empty::<String>());
    b.upsert(
        "s1",
        upsert("items")
            .key(json!({"id": "a"}))
            .value(doc! { "id" => "a", "qty" => 3 }),
    );
    b.upsert(
        "s2",
        upsert("items")
            .key(json!({"id": "b"}))
            .value(doc! { "id" => "b", "qty" => 1 }),
    );
    b.upsert(
        "s3",
        upsert("items")
            .key(json!({"id": "c"}))
            .value(doc! { "id" => "c", "qty" => 4 }),
    );
    b.upsert(
        "s4",
        upsert("items")
            .key(json!({"id": "d"}))
            .value(doc! { "id" => "d", "qty" => 1 }),
    );
    b.upsert(
        "s5",
        upsert("items")
            .key(json!({"id": "e"}))
            .value(doc! { "id" => "e", "qty" => 5 }),
    );
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
    assert_eq!(qr.records[0].get("qty").and_then(|v| v.as_i64()), Some(5));
    assert_eq!(qr.records[1].get("qty").and_then(|v| v.as_i64()), Some(4));

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
        db.create_table("main", "orders").expect("add orders table");
    }
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // Seed users + orders.
    let mut b = Batch::new();
    b.id("seed");
    b.return_only(std::iter::empty::<String>());
    b.upsert(
        "u1",
        upsert("users")
            .key(json!({"id": 1}))
            .value(doc! { "id" => 1, "name" => "alice" }),
    );
    b.upsert(
        "u2",
        upsert("users")
            .key(json!({"id": 2}))
            .value(doc! { "id" => 2, "name" => "bob" }),
    );
    b.upsert(
        "o1",
        upsert("orders")
            .key(json!({"id": 100}))
            .value(doc! { "id" => 100, "user_id" => 1, "amt" => 9 }),
    );
    b.upsert(
        "o2",
        upsert("orders")
            .key(json!({"id": 101}))
            .value(doc! { "id" => 101, "user_id" => 1, "amt" => 4 }),
    );
    b.upsert(
        "o3",
        upsert("orders")
            .key(json!({"id": 102}))
            .value(doc! { "id" => 102, "user_id" => 2, "amt" => 7 }),
    );
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

    let mut b = Batch::new();
    b.id("ddl");
    b.create_table("mk", ddl::create_table("inventory").repo("main"));
    let admin = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&admin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch from create_table, got {:?}", other),
    };
    assert!(resp.results.contains_key("mk"), "mk result present");

    // Verify table is usable: insert + read back.
    let mut b = Batch::new();
    b.id("rw");
    b.upsert(
        "ins",
        upsert("inventory")
            .key(json!({"sku": "X1"}))
            .value(doc! { "sku" => "X1", "stock" => 42 }),
    );
    b.query("rd", Query::from("inventory"));
    let rw = execute_built("prod", b.build());
    let res2 = decode(
        &handler
            .handle(&session, &encode(&rw), &ConnectionServices::without_push(0))
            .await
            .unwrap(),
    );
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

    let mut b = Batch::new();
    b.id("ddl");
    b.drop_table("drop", ddl::drop_table("items").repo("main"));
    let admin = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&admin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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

    let mut b = Batch::new();
    b.id("client-correlation-token-42");
    b.query("all", Query::from("items"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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
// Server-side query-limits cap — operator's max wins over client payload.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_query_limits_cap_clamps_max_queries() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    // Hard cap at 2 queries per batch — way below the client default of 50.
    let handler = ShamirDbHandler::new(shamir).with_query_limits(QueryLimitsCap {
        max_result_size_bytes: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_queries_per_batch: 2,
    });
    let session = user_session();

    // Client sends 5 queries with the default `BatchLimits` (max_queries=50).
    // Server-side cap clamps to 2 → planner returns TooManyQueries with
    // max=2, not max=50.
    let mut b = Batch::new();
    b.id("v");
    b.query("q1", Query::from("items"));
    b.query("q2", Query::from("items"));
    b.query("q3", Query::from("items"));
    b.query("q4", Query::from("items"));
    b.query("q5", Query::from("items"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "limits", "TooManyQueries → 'limits' code");
            assert!(
                message.contains("max: 2"),
                "error message must surface the SERVER cap (2), not the client default (50); got {:?}",
                message
            );
        }
        other => panic!("expected limits error, got {:?}", other),
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
    let mut b = Batch::new();
    b.id("v");
    b.query("all", Query::from("items"));
    let req = execute_built_with_version("prod", 99, b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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

    let mut b = Batch::new();
    b.id("v");
    b.query("all", Query::from("items"));
    let req = execute_built_with_version(
        "prod",
        shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        b.build(),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    assert!(
        matches!(res, DbResponse::Batch { .. }),
        "current version must be accepted; got {:?}",
        res
    );
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
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    let session = user_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec![],
    };
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
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
        AdminGlue {
            user_dir: user_dir.clone(),
            kdf: fast_kdf(),
            tables_registry: None,
        },
    );
    let session = root_session();

    let req = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "correct horse battery staple".into(),
        roles: vec!["read_write".into()],
    };
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::UserCreated { name, user_id } => {
            assert_eq!(name, "bob");
            assert_eq!(user_id.len(), 16, "user_id is a stable 16-byte handle");
        }
        other => panic!("expected UserCreated, got {:?}", other),
    }

    use shamir_connect::server::admin::UserDirectory;
    assert!(
        user_dir.lookup_by_name("bob").is_some(),
        "persisted in directory"
    );
    let roles = user_dir.lookup_roles("bob").unwrap().unwrap_or_default();
    assert!(roles.iter().any(|r| r == "read_write"), "roles attached");

    // Second insert with same name -> typed user_exists error.
    let req2 = DbRequest::CreateScramUser {
        name: "bob".into(),
        password: "another password".into(),
        roles: vec![],
    };
    let res2 = decode(
        &handler
            .handle(
                &session,
                &encode(&req2),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "user_exists"),
        other => panic!("expected user_exists, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Shomer DAC: wire-level enforcement through session_actor → execute_as →
// authorize_access → permits.
// --------------------------------------------------------------------------

/// Build a non-superuser session with a specific username.
///
/// `principal_id()` = `fxhash::hash64(username) & (i64::MAX as u64)`, which
/// must differ from the resource owner id set via `set_resource_meta` for the
/// deny path to trigger.
fn named_user_session(username: &str) -> Session {
    Session::new(
        [0xCC; 16],
        username.into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

/// Shomer DAC end-to-end through the handler wire path.
///
/// Proves that `session_actor` → `ShamirDb::execute_as` → `authorize_access`
/// → `permits` is live on the `ShamirDbHandler::execute` entry point:
///
///   1. Seed a table as System (superuser session).
///   2. `set_resource_meta` to owner=User(1), mode=0o700 (owner-only).
///   3. A non-superuser session whose `principal_id()` != 1 is DENIED.
///   4. A superuser session (Actor::System) is ALLOWED.
///
/// If `session_actor` were removed (always System) or `execute_as` skipped
/// the `authorize_access` call, assertion (3) would fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shomer_dac_denies_non_owner_through_handler_wire() {
    use shamir_db::access::{Actor, ResourceMeta, ResourcePath};

    let shamir = make_db_with_table("acl", "main", "secret").await;

    // Seed a row so Read has something to return on the allow path.
    let mut b = Batch::new();
    b.id("seed");
    b.return_only(std::iter::empty::<String>());
    b.upsert(
        "s",
        upsert("secret")
            .key(json!({"id": 1}))
            .value(doc! { "id" => 1, "data" => "classified" }),
    );
    shamir.execute("acl", &b.build()).await.expect("seed");

    // Restrict the table: owner=User(1), mode=0o700 (owner rwx, nobody else).
    shamir
        .set_resource_meta(
            &ResourcePath::table("acl", "main", "secret"),
            &ResourceMeta {
                owner: Actor::User(1),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .expect("set_resource_meta");

    let handler = ShamirDbHandler::new(shamir);

    // --- Non-owner, non-superuser session → DENIED ---
    // "eve" hashes to a principal_id that is NOT 1.
    let eve = named_user_session("eve");
    assert_ne!(
        eve.principal_id(),
        1,
        "eve's principal_id must differ from owner 1"
    );

    let mut b = Batch::new();
    b.id("rd");
    b.query("r", Query::from("secret"));
    let read = execute_built("acl", b.build());
    let res = decode(
        &handler
            .handle(&eve, &encode(&read), &ConnectionServices::without_push(0))
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "non-owner should be denied; got code={code}, msg={message}"
            );
            assert!(
                message.contains("access denied"),
                "error message should describe denial: {message}"
            );
        }
        other => panic!("expected access_denied error for eve, got {:?}", other),
    }

    // --- Superuser session (Actor::System) → ALLOWED ---
    let su = root_session();
    let res = decode(
        &handler
            .handle(&su, &encode(&read), &ConnectionServices::without_push(0))
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Batch { response } => {
            let rows = &response.results.get("r").expect("r alias").records;
            assert_eq!(rows.len(), 1, "superuser should see the seeded row");
        }
        other => panic!("expected Batch for superuser, got {:?}", other),
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

// --------------------------------------------------------------------------
// Phase B Stage 8 — per-tx staging byte budget (`tx_too_large` abort).
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_too_large_aborts_and_removes_handle() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    // Tiny cap (32 bytes) — a single insert of a non-trivial row blows past it.
    let handler = ShamirDbHandler::new(shamir).with_tx_limits(TxLimitsCap { max_tx_bytes: 32 });
    let session = user_session();

    // BEGIN
    let begin = DbRequest::TxBegin {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        repo: "main".into(),
        isolation: None,
    };
    let opened = decode(
        &handler
            .handle(
                &session,
                &encode(&begin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let tx_handle = match opened {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened, got {:?}", other),
    };

    // EXECUTE — insert one row (well above 32 bytes once MessagePack-encoded).
    let mut b = Batch::new();
    b.id("v");
    b.insert(
        "ins",
        insert("items").row(doc! {
            "name" => "Alice the Great",
            "tag" => "padding-to-blow-past-32-bytes",
        }),
    );
    let body = b.build();
    let exec = DbRequest::TxExecute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        tx_handle,
        batch: body,
    };
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&exec),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "tx_too_large",
                "expected tx_too_large, got {:?}",
                message
            );
            assert!(
                message.contains("max_tx_bytes"),
                "msg should name the cap: {:?}",
                message
            );
        }
        other => panic!("expected tx_too_large Error, got {:?}", other),
    }

    // Verify the handle is gone — a follow-up COMMIT on it should now
    // surface `tx_not_found`, proving the abort removed it from the registry.
    let commit = DbRequest::TxCommit {
        db: "prod".into(),
        tx_handle,
    };
    let after = decode(
        &handler
            .handle(
                &session,
                &encode(&commit),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match after {
        DbResponse::Error { code, .. } => assert_eq!(code, "tx_not_found"),
        other => panic!("expected tx_not_found after abort, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_under_cap_passes_through() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    // Generous cap — same insert as above must succeed.
    let handler = ShamirDbHandler::new(shamir).with_tx_limits(TxLimitsCap {
        max_tx_bytes: 64 * 1024 * 1024,
    });
    let session = user_session();

    let begin = DbRequest::TxBegin {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        repo: "main".into(),
        isolation: None,
    };
    let tx_handle = match decode(
        &handler
            .handle(
                &session,
                &encode(&begin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    ) {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected TxOpened, got {:?}", other),
    };
    let mut b = Batch::new();
    b.id("v");
    b.insert("ins", insert("items").row(doc! { "name" => "Alice" }));
    let body = b.build();
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&DbRequest::TxExecute {
                    query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
                    db: "prod".into(),
                    tx_handle,
                    batch: body,
                }),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    assert!(matches!(res, DbResponse::TxBatch { .. }), "got {:?}", res);
}
