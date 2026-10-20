//! Shomer DAC wire enforcement and per-tx staging byte budget (`tx_too_large`).

use shamir_types::mpack;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::{insert, upsert};
use shamir_query_builder::Query;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler, TxLimitsCap};

use crate::common::{
    decode, encode, execute_built, make_db_with_table, root_session, user_session,
};

/// Build a non-superuser session with a specific username.
///
/// `principal64(session.user_id)` projects the session's directory-minted
/// 16-byte user_id (`[0xCC; 16]` here) into a stable u64, which must differ
/// from the resource owner id set via `set_resource_meta` for the deny path
/// to trigger.
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
///   3. A non-superuser session whose `principal64(user_id)` != 1 is DENIED.
///   4. A superuser session (Actor::Admin) is ALLOWED.
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
    b.try_upsert(
        "s",
        upsert("secret")
            .key(mpack!({"id": 1}))
            .value(doc! { "id" => 1, "data" => "classified" }),
    )
    .unwrap();
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
    // "eve" projects (via principal64 over her session's directory-minted
    // user_id bytes [0xCC;16]) to a principal id that is NOT 1.
    let eve = named_user_session("eve");
    assert_ne!(
        shamir_types::access::principal64(eve.user_id),
        1,
        "eve's principal64 must differ from owner 1"
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
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        visibility_wait: None,
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
