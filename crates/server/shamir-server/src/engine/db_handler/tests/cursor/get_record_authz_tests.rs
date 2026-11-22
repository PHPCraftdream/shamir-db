//! Point-read authorization-binding and name-bound tests for
//! `DbRequest::GetRecord` (admin-shell follow-up).
//!
//! Covers the TOCTOU window the point read had when it authorized names,
//! parked on the global response byte budget, and then resolved those names
//! again: a concurrent ACL revocation or a drop+recreate of the table could
//! slip in while the request was parked. Also covers the cold-resolver
//! retention defect (an unregistered table name must never reach
//! `ShamirDb::get_table`) and the caller-name length bound.
//!
//! The park rendezvous is deterministic and sleep/yield free: the test holds
//! the whole byte budget, and the session's single request slot is the
//! observable — the handler takes it (step 3 of `get_record`) and then runs
//! the synchronous resolve/`has_table`/incarnation capture (step 5) before
//! its next suspension at the budget acquire (step 6), so "slot taken"
//! implies "pre-budget resolution finished". The read future is polled
//! directly (not spawned), so nothing can race the mutation.
//!
//! Fixture mirrors `get_record_tests.rs` (in-memory `ShamirDb`, owner alice,
//! repo `main` with table `items`, one seeded row; requests built ONLY
//! through `shamir_query_builder` and dispatched through the real handler;
//! responses decoded as wire types). Every await is bounded by a 120 s
//! timeout (the Windows CI runner may be up to 10x slower).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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
use shamir_query_builder::ddl::{create_table, drop_table};
use shamir_query_builder::doc;
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex};
use shamir_query_types::read::QueryRecord;
use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_types::types::record_id::RecordId;

use crate::byte_budget::ByteBudget;
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const ALICE_SID: [u8; 32] = [0xAA; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The byte budget the TEST holds for the whole parked window. The handler's
/// `max_result_size_bytes` is deliberately bigger, so its upfront
/// reservation cannot fit while the test holds this slice.
const SMALL_BUDGET: usize = 64;
/// The handler result cap used by the parked fixtures (bigger than
/// `SMALL_BUDGET`, bigger than the seeded record's encoded size).
const PARKED_RESULT_CAP: usize = 1024;
/// Payload value of the seeded control row.
const SEEDED_V: i64 = 42;

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

/// Superuser-class alice: `session_actor` maps `is_superuser` to
/// `Actor::Admin`, which short-circuits `authorize_access` — used ONLY
/// where the test's subject is name RESOLUTION, not ACL.
fn superuser_alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

fn parked_limits() -> QueryLimitsCap {
    QueryLimitsCap {
        max_result_size_bytes: PARKED_RESULT_CAP,
        max_in_flight_requests_per_session: 1,
        ..QueryLimitsCap::UNLIMITED
    }
}

/// Handler over an in-memory `ShamirDb` with `app.main.items` owned by alice
/// (principal `[0xAB; 16]`), plus one seeded row. Returns the handler and the
/// seeded row's storage `RecordId`.
async fn build_handler(
    byte_budget: ByteBudget,
    query_limits: QueryLimitsCap,
) -> (ShamirDbHandler, RecordId) {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner))
        .await
        .expect("add repo must complete within the 120s bound")
        .expect("add repo");

    let handler = ShamirDbHandler::new(Arc::new(shamir))
        .with_query_limits(query_limits)
        .with_byte_budget(byte_budget);
    let seeded = seed_row(&handler).await;
    (handler, seeded)
}

/// Insert the control row through the ENGINE's real `execute_as` path and
/// return its STORAGE `RecordId` (never a payload field). The insert stays
/// below the fixture's budget/caps: it does not go through the handler.
async fn seed_row(handler: &ShamirDbHandler) -> RecordId {
    let mut b = Batch::new();
    b.insert(
        "seed",
        insert("items").row(doc! { "id" => "k000", "v" => SEEDED_V }),
    );
    let response = tokio::time::timeout(
        CALL_TIMEOUT,
        handler.db().execute_as(Actor::System, "app", &b.build()),
    )
    .await
    .expect("engine insert must complete within the 120s bound")
    .expect("engine insert must succeed");
    let result = response.results.get("seed").expect("insert alias returned");
    match result.records.first() {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    }
}

/// Send a wire request through the REAL `RequestHandler::handle`, bounded by
/// `CALL_TIMEOUT`.
async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = tokio::time::timeout(
        CALL_TIMEOUT,
        Box::pin(handler.handle(session, &bytes, &conn)),
    )
    .await
    .expect("handle must complete within the 120s bound")
    .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

fn get_record_req(key: RecordId) -> DbRequest {
    get_record("app", "main", "items", key)
}

fn expect_error(resp: DbResponse, code: &str, context: &str) {
    match resp {
        DbResponse::Error { code: got, .. } => assert_eq!(got, code, "{context}"),
        other => panic!("{context}: expected error code '{code}', got {other:?}"),
    }
}

/// Drive the point read until it is parked at the upfront budget reservation
/// with the pre-budget resolution already done. The session's single request
/// slot is the observable: while we can still take it ourselves the child has
/// not reached step 3, so poll the child once (it must stay Pending, the
/// budget is held); once we cannot take it, the child took it and ran the
/// synchronous step 4/5 code in the same poll, so the capture is done.
async fn drive_to_capture<F: Future<Output = DbResponse>>(
    read: &mut Pin<Box<F>>,
    session: &Session,
) {
    // The spin bound turns an unexpected liveness bug into a fast, clear
    // failure instead of a hang (a healthy in-memory read advances or parks
    // in a handful of polls).
    let mut polls = 0u32;
    loop {
        if let Some(probe) = session.try_acquire_request_slot(1) {
            drop(probe);
            let polled =
                std::future::poll_fn(|cx| std::task::Poll::Ready(Future::poll(read.as_mut(), cx)))
                    .await;
            assert!(
                polled.is_pending(),
                "the point read must park on the held budget, not complete"
            );
            polls += 1;
            assert!(
                polls <= 100_000,
                "the point read never reached the budget park"
            );
        } else {
            return;
        }
    }
}

/// Chmod `app.main.items` to `mode` as the System actor (the same ACL mutation
/// style the existing point-read tests use).
async fn chmod_items_table(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "table",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        )
        .hmac("get-record-authz-test")
        .build()
        .expect("build chmod table"),
    );
    tokio::time::timeout(
        CALL_TIMEOUT,
        handler
            .db()
            .execute_as(Actor::System, "app", &batch.build()),
    )
    .await
    .expect("admin chmod must complete within the 120s bound")
    .expect("admin chmod must succeed");
}

/// Drop `app.main.items` and create a fresh `items` in the same repo, as
/// alice, through the REAL handler dispatch. A second handler over the SAME
/// `ShamirDb` is used with an unbounded budget: dispatching the DDL on the
/// bounded handler would park it on the very reservation the test holds (its
/// upfront reserve is `batch.limits.max_result_size`, 10 MB by default), which
/// would deadlock the rendezvous.
async fn replace_items_table(handler: &ShamirDbHandler) {
    let session = alice_session();
    let unsigned = drop_table("items")
        .repo("main")
        .build()
        .expect("build unsigned drop table");
    let tag = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &unsigned).expect("drop-table canonical form"),
    );
    let drop_op = drop_table("items")
        .repo("main")
        .hmac(tag)
        .build()
        .expect("build signed drop table");

    let mut drop_batch = Batch::new();
    drop_batch.drop_table("destroy", drop_op);
    let mut create_batch = Batch::new();
    create_batch.create_table("recreate", create_table("items").repo("main"));

    let ddl_handler =
        ShamirDbHandler::new(Arc::clone(handler.db())).with_byte_budget(ByteBudget::unbounded());

    // Two separate dispatches: one batch would leave the drop/create order
    // to the batch executor, and a create-before-drop would merely fail.
    let drop_resp = send(
        &ddl_handler,
        &session,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: drop_batch.build(),
        },
    )
    .await;
    assert!(
        matches!(drop_resp, DbResponse::Batch { .. }),
        "drop through the handler must succeed, got {drop_resp:?}"
    );

    let create_resp = send(
        &ddl_handler,
        &session,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: create_batch.build(),
        },
    )
    .await;
    assert!(
        matches!(create_resp, DbResponse::Batch { .. }),
        "create through the handler must succeed, got {create_resp:?}"
    );
}

// ---------------------------------------------------------------------------
// P1#1 — authorization must stay bound to the incarnation that is read.
// ---------------------------------------------------------------------------

/// The read passes the early ACL, parks on the held budget, the table's Read
/// right is revoked, the budget is released: the reply must be the typed
/// `access_denied`, never a record payload.
///
/// WITHOUT the fix the handler never re-authorizes, so the parked read
/// resolves the table and returns the seeded `DbResponse::Record`.
#[tokio::test]
async fn get_record_refuses_when_table_read_is_revoked_while_parked_on_budget() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());
    assert_ne!(session.activity_handle(), 0, "store-stamped session");

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(&handler, &session, get_record_req(seeded)));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    // Mutate while the read is parked: revoke alice's Table Read.
    chmod_items_table(&handler, 0o000).await;
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    expect_error(
        resp,
        "access_denied",
        "table Read revoked while parked on the budget",
    );
}

/// The read passes the early ACL and parks on the held budget; the table is
/// then dropped and recreated under the SAME name in the same repo. The name
/// is configured and the ACL still passes, so only the captured `store_id`
/// comparison can refuse it — with the same typed `access_denied`.
///
/// WITHOUT the fix the read resolves the NEW (empty) table and returns
/// `not_found` instead of the refusal.
#[tokio::test]
async fn get_record_refuses_when_the_authorized_table_is_replaced_while_parked() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, _seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(
        &handler,
        &session,
        get_record_req(RecordId([7u8; 16])),
    ));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    replace_items_table(&handler).await;
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    expect_error(
        resp,
        "access_denied",
        "authorized table incarnation replaced while parked",
    );
}

/// Control: identical park-and-release with NO mutation returns the seeded
/// record (the parking machinery itself must not refuse anything).
#[tokio::test]
async fn get_record_granted_control_returns_the_seeded_row_after_parking() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(&handler, &session, get_record_req(seeded)));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    match resp {
        DbResponse::Record { record } => {
            assert_eq!(
                record.get_value_i64("v"),
                Some(SEEDED_V),
                "the seeded row's payload must come back"
            );
        }
        other => panic!("expected the seeded Record, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// P1#2 — an unregistered table name must not reach the cold table resolver.
// ---------------------------------------------------------------------------

/// Eight DISTINCT never-created table names, each a typed `unknown_table`,
/// must leave the repository's direct cell map byte-for-byte unchanged:
/// `ShamirDb::get_table` would otherwise insert one catalogue cell per probe
/// before checking configuration.
///
/// WITHOUT the fix these eight requests grow the cell map by eight.
#[tokio::test]
async fn missing_table_names_do_not_grow_the_catalogue_cells() {
    let (handler, _seeded) =
        build_handler(ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = superuser_alice_session();

    let repo = handler
        .db()
        .get_db("app")
        .expect("app database")
        .get_repo("main")
        .expect("main repo");
    let before = repo.table_cell_count();

    for i in 0..8u8 {
        let name = format!("absent_table_{i}");
        let resp = send(
            &handler,
            &session,
            get_record("app", "main", name.as_str(), RecordId([9u8; 16])),
        )
        .await;
        expect_error(resp, "unknown_table", "unregistered table name");
    }

    assert_eq!(
        repo.table_cell_count(),
        before,
        "a refused missing-table probe must not insert a catalogue cell"
    );
}

// ---------------------------------------------------------------------------
// P2#3 — bound every caller-supplied name before it can be echoed/resolved.
// ---------------------------------------------------------------------------

fn expect_invalid_name(resp: DbResponse, context: &str) {
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "invalid_request", "{context}");
            assert_eq!(
                message, "db, repo and table names must be 1..=255 UTF-8 bytes",
                "{context}"
            );
            assert!(
                !message.contains("aaaa"),
                "{context}: the refusal must not echo the caller's name"
            );
        }
        other => panic!("{context}: expected the fixed invalid_request refusal, got {other:?}"),
    }
}

/// A 256-byte `db`, `repo` or `table` name is the fixed `invalid_request`
/// refusal that never echoes the input; a 255-byte table name passes the
/// bound (it then fails later with `unknown_table`, proving the boundary).
///
/// WITHOUT the fix the over-long name is echoed back in a resolution error
/// and reaches the catalogue/resolver instead of being refused up front.
#[tokio::test]
async fn get_record_refuses_over_long_names_without_echoing_them() {
    let (handler, _seeded) =
        build_handler(ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = superuser_alice_session();
    let key = RecordId([9u8; 16]);

    let over_long = "a".repeat(256);
    let resp = send(
        &handler,
        &session,
        get_record(over_long.as_str(), "main", "items", key),
    )
    .await;
    expect_invalid_name(resp, "256-byte db name");

    let resp = send(
        &handler,
        &session,
        get_record("app", over_long.as_str(), "items", key),
    )
    .await;
    expect_invalid_name(resp, "256-byte repo name");

    let resp = send(
        &handler,
        &session,
        get_record("app", "main", over_long.as_str(), key),
    )
    .await;
    expect_invalid_name(resp, "256-byte table name");

    // Boundary: 255 bytes is accepted by the name bound and then refused by
    // the configured-table check.
    let at_limit = "a".repeat(255);
    let resp = send(
        &handler,
        &session,
        get_record("app", "main", at_limit.as_str(), key),
    )
    .await;
    expect_error(
        resp,
        "unknown_table",
        "255-byte table name passes the bound",
    );
}
