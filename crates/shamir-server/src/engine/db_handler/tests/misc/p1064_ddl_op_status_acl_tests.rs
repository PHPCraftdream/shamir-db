//! #1064 (P0 SECURITY) — `DbRequest::GetDdlOpStatus` must be authorized.
//!
//! Before this fix, `ShamirDbHandler::get_ddl_op_status` ran with NO
//! authorization at all — any authenticated client that knew (or guessed,
//! or found in a log/metric/bug-report) a valid `op_id` could poll another
//! actor's DDL status, including `Failed.detail` internal error text, and
//! use the found/not-found distinction as a db/repo/table existence oracle.
//!
//! Mirrors `cursor_handler_tests.rs`'s harness style (wire-level
//! `RequestHandler::handle` round trip, real in-memory `ShamirDb`).

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::db_handler::handler::ShamirDbHandler;

const ALICE_SID: [u8; 32] = [0xAA; 32];
const BOB_SID: [u8; 32] = [0xBB; 32];

/// Owner of `app.main.items` and its index -- authorized to poll DDL status
/// on it.
fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

/// A different, unrelated, non-superuser actor -- NOT granted any access to
/// `app.main.items`.
fn bob_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = BOB_SID;
    s
}

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`,
/// owned by alice, with a regular index already created and then dropped --
/// minting a REAL `op_id` via `execute_as` directly (server-internal, same
/// as how `cursor_handler_tests.rs::build_handler_with_rows` seeds rows;
/// bypasses the wire-level HMAC/coarse-admin-gate concerns that are
/// orthogonal to what THIS test authorizes: the poll, not the DDL op
/// itself). Returns the handler and the minted `op_id` string.
async fn build_handler_with_dropped_index() -> (ShamirDbHandler, String) {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    let table = shamir
        .get_db("app")
        .expect("db exists")
        .get_table("main", "items")
        .await
        .expect("table exists");
    table
        .create_index("idx_city", &["city"])
        .await
        .expect("create index");

    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("idx_city", "items").repo("main"));
    let batch = b.build();
    let resp = shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("drop index");
    let op_id = resp.results["d"]
        .op_id
        .as_ref()
        .expect("DROP INDEX must mint an op_id")
        .to_string();

    (ShamirDbHandler::new(Arc::new(shamir)), op_id)
}

async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = handler
        .handle(session, &bytes, &conn)
        .await
        .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

fn get_ddl_op_status_req(op_id: &str) -> DbRequest {
    DbRequest::GetDdlOpStatus {
        db: "app".to_string(),
        repo: "main".to_string(),
        table: "items".to_string(),
        op_id: op_id.to_string(),
    }
}

/// An actor with NO access to the table must NOT be able to poll a REAL,
/// correct `op_id` for that table's DDL status -- this is the exact defect
/// #1064 fixes.
#[tokio::test]
async fn get_ddl_op_status_denies_unauthorized_actor_with_correct_op_id() {
    let (handler, op_id) = build_handler_with_dropped_index().await;
    let bob = bob_session();

    let resp = send(&handler, &bob, get_ddl_op_status_req(&op_id)).await;

    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "bob has no grant on app.main.items -- must be denied, not served a status"
            );
            // The type-level guarantee IS the no-leak proof: `DbResponse::Error`
            // carries no `DdlOpStatus` payload at all (unlike the success
            // variant, `DbResponse::DdlOpStatus { status }`) -- there is no
            // `Failed.detail` (or any other status field) reachable from this
            // branch for a denied caller, structurally, not just by convention.
        }
        other => panic!("expected access_denied, got {other:?}"),
    }
}

/// The SAME unauthorized actor polling a well-formed but NON-EXISTENT
/// op_id must get the EXACT SAME error shape (code) as polling the real
/// one -- proving the response does not function as an existence oracle
/// for an unauthorized caller.
#[tokio::test]
async fn get_ddl_op_status_unauthorized_response_indistinguishable_from_unknown_op_id() {
    let (handler, real_op_id) = build_handler_with_dropped_index().await;
    let bob = bob_session();

    let real_resp = send(&handler, &bob, get_ddl_op_status_req(&real_op_id)).await;

    // A well-formed (valid RecordId string shape) but never-minted op_id.
    let fake_op_id = shamir_types::types::record_id::RecordId::system("never_existed").to_string();
    let fake_resp = send(&handler, &bob, get_ddl_op_status_req(&fake_op_id)).await;

    let real_code = match &real_resp {
        DbResponse::Error { code, .. } => code.clone(),
        other => panic!("expected access_denied for the real op_id, got {other:?}"),
    };
    let fake_code = match &fake_resp {
        DbResponse::Error { code, .. } => code.clone(),
        other => panic!("expected access_denied for the fake op_id, got {other:?}"),
    };

    assert_eq!(
        real_code, fake_code,
        "an unauthorized actor's response code must be identical whether the \
         op_id is real or fake -- otherwise the response is an existence oracle"
    );
    assert_eq!(
        real_code, "access_denied",
        "both must be denied at the authorization gate, before op_id resolution"
    );
}

/// Regression guard: the actual owner (authorized) can still poll DDL
/// status normally after the fix -- the gate must not have broken the
/// legitimate path.
#[tokio::test]
async fn get_ddl_op_status_allows_authorized_owner() {
    let (handler, op_id) = build_handler_with_dropped_index().await;
    let alice = alice_session();

    let resp = send(&handler, &alice, get_ddl_op_status_req(&op_id)).await;

    match resp {
        DbResponse::DdlOpStatus { status } => {
            assert!(
                status.is_some(),
                "alice (owner) must see the real DDL status for her own op_id"
            );
        }
        other => panic!("expected DdlOpStatus, got {other:?}"),
    }
}
