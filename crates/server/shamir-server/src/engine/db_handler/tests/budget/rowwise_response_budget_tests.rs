//! Encoded error strings are unbounded too; refusal must not retain them.

use std::sync::Arc;

use shamir_connect::common::kdf_params::KdfParams;
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
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest};
use shamir_query_types::hmac as canon;
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::byte_budget::{
    run_with_guard_slot, take_stashed_guard, take_stashed_serialized_response, ByteBudget,
};
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::ports::DirectoryPorts;
use crate::user_directory::{FjallUserDirectory, ScramDerivationGate};
use crate::version::CURRENT_QUERY_LANG_VERSION;
use tokio_util::task::AbortOnDropHandle;

// Red before W10: bounded mode grows and stashes this oversized error;
// unbounded mode never checks its encoded size at all. An uncoded INSERT miss
// keeps the engine's table-derived diagnostic (READ misses are canonicalized).
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
        // An INSERT miss is deliberately uncoded: unlike a READ miss, the
        // engine keeps its table-derived diagnostic (see query_runner.rs).
        batch.insert("r", insert("t".repeat(10_240)).row(doc! { "id" => "k1" }));
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
            match &response {
                DbResponse::Error { code, message } => {
                    assert_eq!(code, "result_too_large", "{message}");
                    let encoded_len = message
                        .strip_prefix("encoded response is ")
                        .and_then(|rest| rest.split_once(" bytes; maximum is 1024"))
                        .and_then(|(len, _)| len.parse::<usize>().ok())
                        .expect("the refusal reports the oversized INSERT diagnostic length");
                    assert!(
                        encoded_len > 1024,
                        "the request-derived diagnostic must exceed the operator cap: {message}",
                    );
                }
                other => panic!("expected the per-response operator-cap refusal, got {other:?}"),
            }
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

/// Run the wire handler in a fresh task-local scope and transfer the reservation
/// to the simulated writer instead of dropping it at scope exit.
async fn dispatch_with_guard(
    handler: &ShamirDbHandler,
    batch: BatchRequest,
) -> (DbResponse, Vec<u8>, crate::byte_budget::ByteBudgetGuard) {
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
            .expect("handle");
        let guard = take_stashed_guard().expect("accepted reply carries writer guard");
        let response = rmp_serde::from_slice(&reply).expect("decode reply");
        (response, reply, guard)
    })
    .await
}

/// B6: multiple fixed refusals keep their upfront reservations through writer
/// completion; a further request parks until enough writer guards are released.
#[tokio::test]
async fn concurrent_fixed_refusals_block_on_writer_reservations() {
    const REFUSAL_ALLOWANCE: usize = 513;
    // The cap must fit the first actual writer charge plus the second
    // refusal's upfront reservation, but not both actual writer charges plus
    // a third upfront reservation. The assertions below verify those
    // relationships against the encoded charges, not an assumed envelope size.
    let cap = 600;
    let budget = ByteBudget::new(Some(cap));
    let handler = seeded_handler(4096).await.with_byte_budget(budget.clone());
    let refusal = || {
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_result_size: 1,
            ..BatchLimits::default()
        });
        batch.query("ignored", Query::from("absent_table"));
        batch.build()
    };
    let assert_refusal = |response: &DbResponse,
                          wire: &[u8],
                          guard: &crate::byte_budget::ByteBudgetGuard| {
        assert!(matches!(response, DbResponse::Error { code, message }
            if code == "unknown_table"
                && message == "table is unavailable"));
        assert!(
            matches!(rmp_serde::from_slice::<DbResponse>(wire).unwrap(), DbResponse::Error { code, message }
            if code == "unknown_table"
                && message == "table is unavailable")
        );
        assert!(wire.len() <= REFUSAL_ALLOWANCE);
        assert!(guard.bytes_reserved() >= wire.len());
        assert!(guard.bytes_reserved() <= REFUSAL_ALLOWANCE);
    };

    let (first, first_wire, first_guard) = dispatch_with_guard(&handler, refusal()).await;
    assert_refusal(&first, &first_wire, &first_guard);
    let first_charge = first_guard.bytes_reserved();
    assert_eq!(budget.used(), first_charge);
    assert!(budget.used() <= cap);

    let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
    let (second_done_tx, mut second_done_rx) = tokio::sync::oneshot::channel();
    let second_handler = handler.clone();
    let second = AbortOnDropHandle::new(tokio::spawn(async move {
        let _ = second_started_tx.send(());
        let result = dispatch_with_guard(&second_handler, refusal()).await;
        let _ = second_done_tx.send(());
        result
    }));
    tokio::time::timeout(std::time::Duration::from_secs(5), second_started_rx)
        .await
        .expect("second dispatch starts")
        .expect("second start signal");
    assert!(cap >= first_charge + REFUSAL_ALLOWANCE);
    let (second_response, second_wire, second_guard) =
        tokio::time::timeout(std::time::Duration::from_secs(5), second)
            .await
            .expect("second refusal completes")
            .expect("second dispatch must not panic");
    assert_refusal(&second_response, &second_wire, &second_guard);
    let second_charge = second_guard.bytes_reserved();
    assert_eq!(budget.used(), first_charge + second_charge);
    assert!(
        budget.used() + REFUSAL_ALLOWANCE > cap,
        "both held writer charges plus a third upfront reservation must exceed the cap",
    );
    assert!(cap >= second_charge + REFUSAL_ALLOWANCE);
    assert!(budget.used() <= cap);
    assert!(matches!(second_done_rx.try_recv(), Ok(())));

    let (third_started_tx, third_started_rx) = tokio::sync::oneshot::channel();
    let (third_done_tx, mut third_done_rx) = tokio::sync::oneshot::channel();
    let third_handler = handler.clone();
    let third = AbortOnDropHandle::new(tokio::spawn(async move {
        let _ = third_started_tx.send(());
        let result = dispatch_with_guard(&third_handler, refusal()).await;
        let _ = third_done_tx.send(());
        result
    }));
    tokio::time::timeout(std::time::Duration::from_secs(5), third_started_rx)
        .await
        .expect("third dispatch starts")
        .expect("third start signal");
    // The current-thread runtime runs the spawned task to its first pending
    // await (the parked reservation) while this task yields; the waiter count
    // is crate-private to the budget's crate, so parking is shown by the
    // unchanged charge and the unfinished dispatch below.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(budget.used(), first_charge + second_charge);
    assert!(matches!(
        third_done_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(budget.used() <= cap);

    drop(first_guard); // simulated socket write completed
    assert_eq!(budget.used(), second_charge);
    drop(second_guard);
    let (third_response, third_wire, third_guard) =
        tokio::time::timeout(std::time::Duration::from_secs(5), third)
            .await
            .expect("third refusal proceeds after writer guards release")
            .expect("third dispatch must not panic");
    assert_refusal(&third_response, &third_wire, &third_guard);
    assert!(matches!(third_done_rx.try_recv(), Ok(())));
    assert_eq!(budget.used(), third_guard.bytes_reserved());
    assert!(budget.used() <= cap);
    drop(third_guard);
    assert_eq!(budget.used(), 0);
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
    batch.query(
        "secret-alias-".to_owned() + &"x".repeat(32),
        Query::from("absent_table"),
    );

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "unknown_table");
            assert_eq!(message, "table is unavailable");
            assert!(!message.contains("secret-alias"));
            let wire = rmp_serde::to_vec_named(&DbResponse::Error { code, message })
                .expect("encode bounded refusal");
            assert!(
                wire.len() <= 513,
                "fixed allowance bound exceeded: {}",
                wire.len()
            );
        }
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
/// The request is an INSERT into a 6 KiB table name (it appears twice in the diagnostic). In
/// `query_runner.rs`, non-READ table-resolution misses retain `error.to_string()`
/// with `code: None`; unlike READs, they are not replaced by `unknown_table`.
/// The diagnostic contains the table name once (plus fixed engine/wrapper
/// text), so its encoded envelope is over the caller's 1-byte cap but below
/// the operator cap. The observed encoded length in the refusal below guards
/// that size relationship directly.
#[tokio::test]
async fn tiny_caller_result_cap_refuses_a_long_non_canonical_diagnostic() {
    let handler = seeded_handler(16_384).await;
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.insert("w", insert("t".repeat(6_144)).row(doc! { "id" => "k1" }));

    match dispatch(&handler, batch.build()).await {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "result_too_large",
                "a caller-derived diagnostic must be fenced by the CALLER's cap, \
                 never transmitted as a canonical refusal: {message}",
            );
            let encoded_len = message
                .strip_prefix("encoded response is ")
                .and_then(|rest| rest.split_once(" bytes; maximum is 1"))
                .and_then(|(len, _)| len.parse::<usize>().ok())
                .expect("the caller-cap refusal reports the original diagnostic envelope size");
            assert!(
                encoded_len > 1,
                "diagnostic must exceed caller cap: {message}"
            );
            assert!(
                encoded_len < 16_384,
                "the request-derived diagnostic must fit the operator cap: {message}",
            );
            assert!(
                message.contains("maximum is 1"),
                "the fence that fired must be the caller's 1-byte cap, not the \
                 operator's: {message}",
            );
        }
        other => panic!("a 6 KiB request-derived diagnostic must be refused, got {other:?}"),
    }
}

/// B6 — a canonical refusal is charged from a fixed allowance reserved before
/// execution; it is never grown after the writer guard has been acquired.
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
            DbResponse::Error { code, message } => {
                assert_eq!(code, "unknown_table");
                assert_eq!(message, "table is unavailable");
            },
            other => panic!("expected the canonical refusal, got {other:?}"),
        }

        let wire = take_stashed_serialized_response().expect("the accepted envelope is stashed");
        let decoded: DbResponse = rmp_serde::from_slice(&wire).expect("decode the stashed reply");
        assert!(
            matches!(&decoded, DbResponse::Error { code, message } if code == "unknown_table" && message == "table is unavailable"),
            "the bytes the writer will transmit must carry the canonical refusal: {decoded:?}",
        );

        let guard = take_stashed_guard().expect("the bounded budget acquires a guard");
        assert!(guard.bytes_reserved() >= wire.len());
        assert!(
            guard.bytes_reserved() <= 513,
            "fixed allowance reservation bound"
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

/// B6 — a native `CreateUser` reaches the post-execution convergence path:
/// the real directory port rejects Argon2 derivation while its permit is held,
/// then `execute_with_rid` encodes/stashes the resulting fixed busy response
/// and keeps the response-budget guard alive for the writer.
#[tokio::test]
async fn create_user_argon2_busy_stays_server_busy_under_one_byte_result_limit() {
    let temp = tempfile::tempdir().expect("user directory tempdir");
    let user_dir = Arc::new(FjallUserDirectory::open(temp.path().join("users")).unwrap());
    let db = ShamirDb::init_memory().await.expect("init shamir");
    db.create_db("app").await.expect("create app");
    let gate = Arc::new(ScramDerivationGate::new(Arc::new(
        shamir_connect::server::argon2_semaphore::Argon2Semaphore::with_capacity(1),
    )));
    let (port, resolver) = DirectoryPorts::new(user_dir.clone(), KdfParams::DEFAULT, gate.clone())
        .into_trait_objects();
    let db = Arc::new(
        db.with_user_admin_port(port)
            .with_principal_resolver(resolver),
    );
    let budget = ByteBudget::new(Some(4608));
    let handler = ShamirDbHandler::with_admin(
        db,
        crate::db_handler::AdminGlue {
            user_dir,
            kdf: KdfParams::DEFAULT,
            tables_registry: None,
            derivation_gate: gate.clone(),
        },
    )
    .with_query_limits(QueryLimitsCap {
        max_result_size_bytes: 8192,
        ..QueryLimitsCap::UNLIMITED
    })
    .with_byte_budget(budget.clone());
    let session = Session::new(
        [0xAB; 16],
        "root".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    let held = gate
        .semaphore()
        .try_acquire()
        .expect("the one Argon2 permit is initially free");

    let op =
        shamir_query_types::batch::BatchOp::CreateUser(shamir_query_types::auth::CreateUserOp {
            create_user: "held_permit_user".into(),
            password: "fixture password".into(),
            roles: Vec::new(),
            database: None,
            hmac: None,
        });
    let tag = canon::compute_tag_hex(
        &session.hmac_key(),
        &canon::canonical_batch_op("app", &op).expect("canonical CreateUser"),
    );
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.create_user(
        "create",
        shamir_query_builder::ddl::create_user("held_permit_user", "fixture password")
            .hmac(&tag)
            .build()
            .expect("build CreateUser"),
    );
    let request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: batch.build(),
    };
    let request_bytes = rmp_serde::to_vec_named(&request).expect("encode Execute");
    let (reply_bytes, guard) = run_with_guard_slot(async {
        let reply = handler
            .handle(
                &session,
                &request_bytes,
                &ConnectionServices::without_push(0),
            )
            .await
            .expect("Execute response encoding");
        let guard = take_stashed_guard().expect("converged response retains its writer guard");
        (reply, guard)
    })
    .await;
    let response: DbResponse = rmp_serde::from_slice(&reply_bytes).expect("decode response");
    assert!(
        matches!(&response, DbResponse::Error { code, message }
            if code == "server_busy" && message == "server_busy"),
        "CreateUser must return the fixed server-owned busy refusal: {response:?}",
    );
    assert!(
        reply_bytes.len() <= 1 + 512,
        "busy refusal fits fixed allowance"
    );
    assert_eq!(budget.used(), guard.bytes_reserved());
    assert_eq!(guard.bytes_reserved(), reply_bytes.len());

    drop(held);
    drop(guard);
    assert_eq!(budget.used(), 0);
}
