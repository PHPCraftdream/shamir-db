//! Health ping, protocol-level errors and request-id correlation.

use std::sync::Arc;

use shamir_db::types::value::QueryValue;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::Query;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};

use crate::common::{decode, encode, execute_built, make_db_with_table, user_session};

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

// --------------------------------------------------------------------------
// FG-5a's compile-safety stub dispatch tests (create/fetch_next/cancel_cursor
// returning the placeholder "cursor_not_yet_implemented" error) were removed
// here once FG-5b replaced the stub with the real cursor registry. Real
// dispatch-through-`ShamirDbHandler::handle` coverage (happy-path pagination,
// snapshot stability against a concurrent write, per-session cap, idle-
// timeout eviction, rejected-temporal scope cut) now lives in
// `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_msgpack_returns_protocol_err() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = user_session();

    let garbage: &[u8] = &[0xff, 0x00, 0x10, 0x42, 0x99, 0x01];
    // Malformed input is now a structured `invalid_request` response, not a
    // transport-level error string.
    let bytes = handler
        .handle(&session, garbage, &ConnectionServices::without_push(0))
        .await
        .expect("invalid request must be a structured response");
    match decode(&bytes) {
        DbResponse::Error { code, .. } => assert_eq!(code, "invalid_request"),
        other => panic!("expected invalid_request Error, got {:?}", other),
    }
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
                QueryValue::Str("client-correlation-token-42".into()),
                "BatchResponse.id should echo BatchRequest.id verbatim"
            );
        }
        other => panic!("expected Batch, got {:?}", other),
    }
}
