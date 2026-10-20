//! `invalid_request` decode responses expose the typed DB error code and
//! preserve the serde diagnostic (including the malformed field/value).

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::wire::{DbRequest, DbResponse};
use std::sync::Arc;

use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

fn session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    )
}

/// Encode a valid `DbRequest::Execute`, round-trip it through
/// `serde_json::Value`, apply `mutate` to the batch object, and re-encode to
/// msgpack. (Typed Rust enums cannot hold typo values like `"syncedd"`, so
/// the wire-level mutation is done generically.)
fn mutated_batch_bytes(mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut b = Batch::new();
    b.id("v");
    b.query("all", Query::from("items"));
    let req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch: b.build(),
    };
    let bytes = rmp_serde::to_vec_named(&req).expect("encode valid request");
    let mut v: serde_json::Value =
        rmp_serde::from_slice(&bytes).expect("valid request round-trips to JSON value");
    mutate(&mut v["batch"]);
    rmp_serde::to_vec(&v).expect("re-encode mutated request")
}

fn decode_err(bytes: &[u8]) -> String {
    match rmp_serde::from_slice::<DbRequest>(bytes) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("payload must be rejected at decode time"),
    }
}

#[test]
fn unknown_batch_key_fails_decode_and_names_the_field() {
    let bytes = mutated_batch_bytes(|batch| {
        batch["zombie_key"] = serde_json::json!(true);
    });
    let err = decode_err(&bytes);
    assert!(
        !err.is_empty(),
        "unknown key must produce a serde diagnostic, got {err:?}"
    );
    assert!(
        err.contains("zombie_key"),
        "serde detail must name the offending field, got {err:?}"
    );
}

#[test]
fn typo_durability_value_fails_decode_and_names_the_value() {
    let bytes = mutated_batch_bytes(|batch| {
        batch["durability"] = serde_json::json!("syncedd");
    });
    let err = decode_err(&bytes);
    assert!(
        err.contains("syncedd"),
        "bad value should be reported: {err:?}"
    );
}

#[test]
fn typo_isolation_value_fails_decode_and_names_the_value() {
    let bytes = mutated_batch_bytes(|batch| {
        batch["isolation"] = serde_json::json!("snapshots");
    });
    let err = decode_err(&bytes);
    assert!(
        err.contains("snapshots"),
        "bad value should be reported: {err:?}"
    );
}

/// End-to-end through the handler: a malformed batch reaches the SAME
/// `invalid_request` decode gate (never a DB-layer response).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_rejects_unknown_batch_key_as_invalid_request() {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let session = session();

    let malformed = mutated_batch_bytes(|batch| {
        batch["zombie_key"] = serde_json::json!(true);
    });
    let response = handler
        .handle(&session, &malformed, &ConnectionServices::without_push(0))
        .await
        .expect("malformed request returns DbResponse::Error");
    let response: DbResponse = rmp_serde::from_slice(&response).expect("decode error response");
    assert!(
        matches!(&response, DbResponse::Error { code, message } if code == "invalid_request" && message.contains("zombie_key"))
    );
}

/// Control: the same envelope WITHOUT the mutation decodes fine and gets a
/// real DB-layer response (here: unknown_db, because `prod` was never
/// created) — proving the rejections above come from decode strictness,
/// not from the envelope being malformed overall.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_unmutated_batch_decodes_and_reaches_db_layer() {
    let bytes = mutated_batch_bytes(|_| {});
    assert!(rmp_serde::from_slice::<DbRequest>(&bytes).is_ok());

    let db = ShamirDb::init_memory().await.expect("init shamir");
    let handler = ShamirDbHandler::new(Arc::new(db));
    let res = handler
        .handle(&session(), &bytes, &ConnectionServices::without_push(0))
        .await
        .expect("valid envelope must decode");
    let res: DbResponse = rmp_serde::from_slice(&res).expect("decode response");
    assert!(
        matches!(&res, DbResponse::Error { code, .. } if code == "unknown_db"),
        "control batch must reach the DB layer (unknown_db), got {res:?}"
    );
}
