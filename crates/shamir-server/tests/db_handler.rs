//! Integration tests for the `RequestHandler` ↔ `ShamirDb` bridge.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::db::engine::table::TableConfig;
use shamir_db::db::ShamirDb;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

fn make_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

async fn make_db_with_table(db: &str, repo: &str, table: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory())
        .add_table(TableConfig::new(table));
    shamir.add_repo(db, cfg).await.expect("add repo");
    Arc::new(shamir)
}

fn encode_req(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

fn decode_resp(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode response")
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_returns_pong() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = make_session();

    let req_bytes = encode_req(&DbRequest::Ping);
    let res_bytes = handler.handle(&session, &req_bytes).expect("handle Ping");
    match decode_resp(&res_bytes) {
        DbResponse::Pong => {}
        other => panic!("expected Pong, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_msgpack_returns_err() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = make_session();

    // Random bytes that cannot decode as DbRequest.
    let garbage: &[u8] = &[0xff, 0x00, 0x10, 0x42, 0x99, 0x01];
    let result = handler.handle(&session, garbage);
    assert!(result.is_err(), "expected Err for garbage input, got {:?}", result);
    let msg = result.unwrap_err();
    assert!(
        msg.starts_with("invalid_request:"),
        "expected invalid_request prefix, got {:?}",
        msg
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_db_returns_error_response() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = make_session();

    let req = DbRequest::Get {
        db: "nope".into(),
        repo: "main".into(),
        table: "users".into(),
        key: serde_json::json!({"id": 1}),
    };
    let res_bytes = handler
        .handle(&session, &encode_req(&req))
        .expect("handle should return Ok with Error payload");
    match decode_resp(&res_bytes) {
        DbResponse::Error { message } => {
            assert!(
                !message.is_empty(),
                "error message should be non-empty"
            );
        }
        other => panic!("expected Error response, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_table_returns_error_response() {
    let shamir = make_db_with_table("prod", "main", "users").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = make_session();

    // Wrong table name.
    let req = DbRequest::Get {
        db: "prod".into(),
        repo: "main".into(),
        table: "missing_table".into(),
        key: serde_json::json!({"id": 1}),
    };
    let res_bytes = handler.handle(&session, &encode_req(&req)).unwrap();
    match decode_resp(&res_bytes) {
        DbResponse::Error { .. } => {}
        other => panic!("expected Error, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tables_returns_table_names() {
    let shamir = make_db_with_table("prod", "main", "users").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = make_session();

    let req = DbRequest::ListTables {
        db: "prod".into(),
        repo: "main".into(),
    };
    let res_bytes = handler.handle(&session, &encode_req(&req)).unwrap();
    match decode_resp(&res_bytes) {
        DbResponse::Tables { names } => {
            assert!(
                names.iter().any(|n| n == "users"),
                "expected `users` in table list, got {:?}",
                names
            );
        }
        other => panic!("expected Tables, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_then_get_round_trip() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = make_session();

    // Set
    let key = serde_json::json!({"id": "k1"});
    let value = serde_json::json!({"id": "k1", "name": "widget", "qty": 7});
    let set_req = DbRequest::Set {
        db: "prod".into(),
        repo: "main".into(),
        table: "items".into(),
        key: key.clone(),
        value: value.clone(),
    };
    let set_bytes = handler.handle(&session, &encode_req(&set_req)).unwrap();
    match decode_resp(&set_bytes) {
        DbResponse::Ok => {}
        other => panic!("expected Ok from Set, got {:?}", other),
    }

    // Get
    let get_req = DbRequest::Get {
        db: "prod".into(),
        repo: "main".into(),
        table: "items".into(),
        key: key.clone(),
    };
    let get_bytes = handler.handle(&session, &encode_req(&get_req)).unwrap();
    match decode_resp(&get_bytes) {
        DbResponse::Value { value: got } => {
            assert_eq!(got.get("id").and_then(|v| v.as_str()), Some("k1"));
            assert_eq!(got.get("name").and_then(|v| v.as_str()), Some("widget"));
            assert_eq!(got.get("qty").and_then(|v| v.as_i64()), Some(7));
        }
        other => panic!("expected Value, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_record() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = make_session();

    // Insert via Set
    let key = serde_json::json!({"id": "to_delete"});
    let value = serde_json::json!({"id": "to_delete", "n": 1});
    let set_req = DbRequest::Set {
        db: "prod".into(),
        repo: "main".into(),
        table: "items".into(),
        key: key.clone(),
        value,
    };
    let _ = handler.handle(&session, &encode_req(&set_req)).unwrap();

    // Delete
    let del_req = DbRequest::Delete {
        db: "prod".into(),
        repo: "main".into(),
        table: "items".into(),
        key: key.clone(),
    };
    let del_bytes = handler.handle(&session, &encode_req(&del_req)).unwrap();
    match decode_resp(&del_bytes) {
        DbResponse::Ok => {}
        other => panic!("expected Ok from Delete, got {:?}", other),
    }

    // Get → not_found
    let get_req = DbRequest::Get {
        db: "prod".into(),
        repo: "main".into(),
        table: "items".into(),
        key,
    };
    let get_bytes = handler.handle(&session, &encode_req(&get_req)).unwrap();
    match decode_resp(&get_bytes) {
        DbResponse::Error { message } => {
            assert_eq!(message, "not_found");
        }
        other => panic!("expected Error not_found, got {:?}", other),
    }
}
