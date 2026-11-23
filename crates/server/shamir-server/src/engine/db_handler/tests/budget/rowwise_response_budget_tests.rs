//! Encoded error strings are unbounded too; refusal must not retain them.

use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest};
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::byte_budget::{
    run_with_guard_slot, take_stashed_guard, take_stashed_serialized_response, ByteBudget,
};
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

// Red before W10: bounded mode grows and stashes this oversized error;
// unbounded mode never checks its encoded size at all.
#[tokio::test]
async fn encoded_oversize_refuses_and_releases_with_or_without_global_budget() {
    for cap in [Some(1024), None] {
        let db = ShamirDb::init_memory().await.unwrap();
        let actor = Actor::User(principal64([0xAB; 16]));
        db.create_db_as("app", actor.clone())
            .await
            .expect("create_db");
        db.add_repo_as(
            "app",
            RepoConfig::new("main", BoxRepoFactory::in_memory()),
            actor,
        )
        .await
        .unwrap();
        let budget = ByteBudget::new(cap);
        let handler = ShamirDbHandler::new(Arc::new(db))
            .with_byte_budget(budget.clone())
            .with_query_limits(QueryLimitsCap {
                max_result_size_bytes: 1024,
                ..QueryLimitsCap::UNLIMITED
            });
        let session = Session::new(
            [0xAB; 16],
            "alice".into(),
            SessionPermissions::from_roles(vec!["read_write".into()]),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
            [0u8; 32],
            UnixNanos::now().as_u64(),
        );
        let conn = ConnectionServices::without_push(0);
        let mut batch = Batch::new();
        batch.query("r", Query::from("x".repeat(10_240)));
        run_with_guard_slot(async {
            let response = handler
                .execute(
                    &session,
                    CURRENT_QUERY_LANG_VERSION,
                    "app",
                    batch.build(),
                    &conn,
                )
                .await;
            assert!(
                matches!(&response, DbResponse::Error { code, .. } if code == "result_too_large"),
                "{response:?}",
            );
            assert_eq!(budget.used(), 0);
            assert!(take_stashed_guard().is_none());
            assert!(take_stashed_serialized_response().is_none());
        })
        .await;
    }
}

// ---------------------------------------------------------------------------
// The CALLER's own `max_result_size` may shrink a DATA reply, never a
// refusal: a coded error envelope keeps its own code.
// ---------------------------------------------------------------------------

fn alice_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// `app.main.items` holding ONE row whose 256-byte `pad` value cannot fit
/// any tiny result cap, served by a handler whose OPERATOR cap
/// (`query_limits.max_result_size_bytes`) is `result_cap` bytes; every other
/// operator cap stays `UNLIMITED`. The seed insert runs on its own unlimited
/// handler — the reply under test is the read that follows.
async fn seeded_handler(result_cap: usize) -> ShamirDbHandler {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner,
    )
    .await
    .expect("repo");
    let db = Arc::new(db);

    let mut seed = Batch::new();
    seed.insert(
        "seed",
        insert("items").row(doc! { "id" => "k0", "pad" => "x".repeat(256) }),
    );
    let seeder = ShamirDbHandler::new(Arc::clone(&db))
        .with_query_limits(QueryLimitsCap::UNLIMITED)
        .with_byte_budget(ByteBudget::unbounded());
    let seeded = seeder
        .execute(
            &alice_session(),
            CURRENT_QUERY_LANG_VERSION,
            "app",
            seed.build(),
            &ConnectionServices::without_push(0),
        )
        .await;
    assert!(
        matches!(seeded, DbResponse::Batch { .. }),
        "seed insert must succeed: {seeded:?}",
    );

    ShamirDbHandler::new(db)
        .with_query_limits(QueryLimitsCap {
            max_result_size_bytes: result_cap,
            ..QueryLimitsCap::UNLIMITED
        })
        .with_byte_budget(ByteBudget::unbounded())
}

/// Dispatch one `Execute` request over the real wire entry point
/// (`RequestHandler::handle`) and decode the reply it would write.
async fn dispatch(handler: &ShamirDbHandler, batch: BatchRequest) -> DbResponse {
    let req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch,
    };
    let req_bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);
    run_with_guard_slot(async {
        let reply = handler
            .handle(&session, &req_bytes, &conn)
            .await
            .expect("handle must not error at the protocol level");
        let _ = take_stashed_guard();
        rmp_serde::from_slice(&reply).expect("decode reply")
    })
    .await
}

/// The caller's own result cap must not destroy the cause of a refusal. A
/// read of an absent table fails with the coded engine error `unknown_table`;
/// with `limits.max_result_size = 1` the encoded-reply cap used to rewrite
/// that envelope into `result_too_large`.
#[tokio::test]
async fn tiny_caller_result_cap_keeps_the_fixed_error_envelope_and_its_code() {
    // The OPERATOR cap comfortably fits the envelope — only the caller's own
    // 1-byte limit makes it look "too large".
    let handler = seeded_handler(4096).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("missing", Query::from("absent_table"));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => assert_eq!(
            code, "unknown_table",
            "a fixed refusal must keep its own code instead of result_too_large: {message}",
        ),
        other => panic!("expected the coded refusal, got {other:?}"),
    }
}

/// Negative control for the exemption above: a DATA reply that really is
/// larger than the caller's cap is STILL refused with `result_too_large`.
/// Without this, "exempt errors" could silently drift into "cap nothing".
#[tokio::test]
async fn tiny_caller_result_cap_still_refuses_a_larger_data_reply() {
    let handler = seeded_handler(4096).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("row", Query::from("items"));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "result_too_large", "{message}");
            assert!(
                message.contains("maximum is 1"),
                "the refusal must name the CALLER's cap, not the operator's: {message}",
            );
        }
        other => panic!("expected the encoded DATA reply to be refused, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// B6: the error-envelope exception is CLOSED to bounded canonical refusals.
// A non-canonical diagnostic (its message embeds caller-supplied text) is
// fenced by the CALLER's cap exactly like a data reply; a canonical refusal
// is fenced by the operator cap AND charged its actual bytes.
// ---------------------------------------------------------------------------

/// B6 — the test that kills the over-broad "every `DbResponse::Error` is
/// exempt up to the operator cap" exception: a LONG, caller-derived
/// diagnostic must NOT slip past a 1-byte caller limit.
///
/// The request is an INSERT into a 10 KiB table name. The engine's
/// `validate_tables` tags only READS with `unknown_table`, so this refusal
/// keeps `code: None` and resolves to the untagged, caller-text-carrying
/// `query` code — a NON-canonical code, exactly what the closed set must
/// leave fenced. The OPERATOR cap (`16_384`) comfortably fits the ~10 KiB
/// message, so ONLY the caller's limit can refuse it: before B6 the reply
/// was transmitted whole (the exception admitted it up to the operator cap),
/// afterwards it is refused with `result_too_large` naming the CALLER's cap.
#[tokio::test]
async fn tiny_caller_result_cap_refuses_a_long_non_canonical_diagnostic() {
    let handler = seeded_handler(16_384).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.insert("w", insert("t".repeat(10_240)).row(doc! { "id" => "k1" }));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "result_too_large",
                "a caller-derived diagnostic must be fenced by the CALLER's cap, \
                 never transmitted as a canonical refusal: {message}",
            );
            assert!(
                message.contains("maximum is 1"),
                "the fence that fired must be the caller's 1-byte cap, not the \
                 operator's: {message}",
            );
        }
        other => panic!("a 10 KiB request-derived diagnostic must be refused, got {other:?}"),
    }
}

/// B6 — accounting: a canonical refusal under a tiny caller limit still
/// REACHES the caller (operator fence), and the bytes it transmits are
/// charged to the bounded global budget before the writer-side release.
///
/// Asserted invariant, in order: the serialized envelope decodes to the
/// canonical `unknown_table` code (it was not rewritten into
/// `result_too_large`); the stashed writer guard's reservation covers the
/// transmitted wire bytes (`bytes_reserved() >= wire.len()` — impossible
/// from the 1-byte upfront reservation alone, so the envelope really was
/// grown/charged); the global budget accounts exactly that reservation
/// while the writer still holds it (`used() == bytes_reserved()`); and after
/// the writer-side guard drops the budget is fully released (`used() == 0`).
#[tokio::test]
async fn bounded_budget_charges_the_canonical_envelope_it_transmits() {
    let cap = 1024;
    let budget = ByteBudget::new(Some(cap));
    let handler = seeded_handler(4096).await.with_byte_budget(budget.clone());
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("missing", Query::from("absent_table"));
    let batch = batch.build();
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    run_with_guard_slot(async {
        let response = handler
            .execute(&session, CURRENT_QUERY_LANG_VERSION, "app", batch, &conn)
            .await;
        match &response {
            DbResponse::Error { code, .. } => assert_eq!(
                code, "unknown_table",
                "a canonical refusal is fenced by the OPERATOR cap, so the 1-byte \
                 caller limit must not refuse it",
            ),
            other => panic!("expected the canonical refusal, got {other:?}"),
        }

        let wire = take_stashed_serialized_response().expect("the accepted envelope is stashed");
        let decoded: DbResponse = rmp_serde::from_slice(&wire).expect("decode the stashed reply");
        assert!(
            matches!(&decoded, DbResponse::Error { code, .. } if code == "unknown_table"),
            "the bytes the writer will transmit must carry the canonical code: {decoded:?}",
        );

        let guard = take_stashed_guard().expect("the bounded budget acquires a guard");
        assert!(
            guard.bytes_reserved() >= wire.len(),
            "the reservation must COVER the transmitted {} bytes, not stay at the \
             1-byte upfront estimate: {} reserved",
            wire.len(),
            guard.bytes_reserved(),
        );
        assert_eq!(
            budget.used(),
            guard.bytes_reserved(),
            "the global budget must account exactly the reservation held for this reply",
        );
        // Writer-side completion: the release happens when the stashed guard
        // drops (in `connection::request_loop`, after the socket write).
        drop(guard);
        assert_eq!(
            budget.used(),
            0,
            "every charged byte must be released when the writer-side guard drops",
        );
        assert!(take_stashed_guard().is_none());
    })
    .await;
}

/// Handler whose OPERATOR result cap is `result_cap` and whose per-session
/// in-flight slot cap is 1, plus a session STAMPED through a
/// [`SessionStore`] — the slot cap is skipped for the handle-0 stateless
/// fallback, so a store-minted (nonzero) activity handle is the
/// precondition for provoking a real `server_busy`.
async fn slot_capped_handler(result_cap: usize) -> (ShamirDbHandler, Arc<Session>) {
    let db = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner,
    )
    .await
    .expect("repo");
    let store = Arc::new(SessionStore::new());
    let mut session_id = [0u8; 32];
    session_id[0] = 0xAB;
    session_id[1] = 0x01;
    let stamped = store.insert(session_id, alice_session());
    assert_ne!(
        stamped.activity_handle(),
        0,
        "store-inserted sessions must carry a minted (stamped) handle",
    );
    let handler = ShamirDbHandler::new(Arc::new(db))
        .with_query_limits(QueryLimitsCap {
            max_result_size_bytes: result_cap,
            max_in_flight_requests_per_session: 1,
            ..QueryLimitsCap::UNLIMITED
        })
        .with_session_store(Arc::clone(&store))
        .with_byte_budget(ByteBudget::unbounded());
    (handler, stamped)
}

/// B6 — a REAL `server_busy` (no synthesized error) survives a 1-byte
/// caller limit.
///
/// The saturation is produced by the real admission path: a second
/// in-flight request holds the session's ONLY slot
/// (`try_track_activity_request`, the same mechanism
/// `activity_request_slot_tests::real_write_is_refused_before_side_effect_when_session_is_saturated`
/// uses), so the `Execute` under test is refused by
/// `try_track_activity_request`'s `Err(())` arm.
///
/// NOTE: that arm is an EARLY `return server_busy_response()` — it leaves
/// `execute_with_rid` BEFORE the RI-15 convergence point, so it was never
/// part of the over-broad error exception and no budget is acquired or
/// charged for it. The assertion is therefore about survival only: the
/// fixed code (and its fixed text) reaches the caller unchanged under a
/// 1-byte caller cap.
#[tokio::test]
async fn real_server_busy_execute_keeps_its_code_under_a_tiny_caller_cap() {
    let (handler, session) = slot_capped_handler(4096).await;
    let held = handler
        .try_track_activity_request(&session, "app", "write", None)
        .expect("the single slot must be free before the saturation");
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.query("row", Query::from("items"));

    let response = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch.build(),
            &ConnectionServices::without_push(0),
        )
        .await;
    match response {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "server_busy",
                "a fixed canonical refusal must keep its code under a tiny cap",
            );
            assert_eq!(message, "server_busy", "the fixed busy text is unchanged");
        }
        other => panic!("a saturated session's execute must be refused, got {other:?}"),
    }

    drop(held);
    assert_eq!(
        handler.activity_requests.in_flight(),
        0,
        "the refusal acquires nothing: the held slot's release empties the table",
    );
}
