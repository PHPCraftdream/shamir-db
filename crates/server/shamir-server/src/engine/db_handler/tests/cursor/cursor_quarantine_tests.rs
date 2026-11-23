//! S5c-X PR8b — cursor reads (`CreateCursor`/`FetchNext`) are gated behind
//! the `resync_required` read-quarantine: a cursor must not be created over
//! a quarantined repo, a cursor created BEFORE the quarantine must fail
//! closed on the next fetch, and healthy / non-replicated repos are
//! unaffected. Fixture mirrors `cursor_handler_tests.rs` (its helpers are
//! module-private, so the minimal setup is duplicated here re-export-free).

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
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::{insert, Insert};
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::read::{QueryRecord, ReadQuery};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use shamir_query_builder::doc;

use shamir_types::types::record_id::RecordId;

use crate::byte_budget::ByteBudget;
use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap};
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const ALICE_SID: [u8; 32] = [0xAA; 32];

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

/// Handler over an in-memory `ShamirDb` with `app/{main,side,other}.items`,
/// owned by alice. `main` gets the quarantine in the tests below; `side`
/// carries an ACTIVE (never quarantined) subscription; `other` has no
/// replication at all.
async fn build_handler() -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    for repo in ["main", "side", "other"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        shamir
            .add_repo_as("app", cfg, owner.clone())
            .await
            .expect("add repo");
    }

    // Seed three rows into each repo's `items` (page_size-1 cursors need a
    // non-empty FIRST page with `has_more == true` — production never
    // registers a cursor whose first page has no more rows — plus rows left
    // for the follow-up FetchNext in tests (b) and (c)).
    let mut b = Batch::new();
    b.id(1);
    for repo in ["main", "side", "other"] {
        for (k, v) in [("k000", 0i64), ("k001", 1i64), ("k002", 2i64)] {
            b.insert(
                format!("i_{repo}_{k}"),
                Insert::with_repo(repo, "items").row(doc! { "id" => k, "v" => v }),
            );
        }
    }
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("seed rows");

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED)
}

/// Create a Pull profile + ACTIVE subscription covering `repo`, then flip it
/// to `resync_required` through the real persisted path
/// (`ShamirDb::mark_subscription_resync_required`, `pub` on ShamirDb — same
/// entry point shamir-db's own `quarantine_reads_tests.rs` uses). The refresh
/// hook rebuilds the read-quarantine cache automatically.
async fn quarantine(handler: &ShamirDbHandler, sub: &str, profile: &str, repo: &str) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile(profile).stream(
            repl_scope("app").repo(repo).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    handler
        .db()
        .execute_as(Actor::System, "app", &b.build())
        .await
        .expect("admin replication batch must succeed");
    handler
        .db()
        .mark_subscription_resync_required(sub, 7, 3)
        .await
        .expect("mark resync_required must succeed");
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

fn create_cursor_req(query: ReadQuery, page_size: u32) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query,
        page_size,
    }
}

fn fetch_next_req(cursor_id: CursorId, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: Some(page_size),
    }
}

fn expect_resync_required(resp: DbResponse, context: &str) {
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "resync_required", "{context}"),
        other => panic!("{context}: expected resync_required error, got {other:?}"),
    }
}

fn expect_first_page(resp: DbResponse) -> CursorId {
    match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(!page.records.is_empty(), "first page must have rows");
            assert!(has_more, "first page must have more rows pending");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
}

/// (a) CreateCursor over a quarantined repo is refused with wire code
/// `resync_required` — no cursor is ever created.
#[tokio::test]
async fn create_cursor_refused_while_repo_quarantined() {
    let handler = build_handler().await;
    let session = alice_session();
    quarantine(&handler, "sub", "prof", "main").await;

    let query = ReadQuery::with_repo("main", "items");
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    expect_resync_required(resp, "CreateCursor over quarantined repo");
}

/// (b) A cursor created over a healthy repo (first page succeeds) fails
/// closed on the next FetchNext once the repo is quarantined mid-scroll.
#[tokio::test]
async fn fetch_next_fails_closed_for_cursor_created_before_quarantine() {
    let handler = build_handler().await;
    let session = alice_session();

    let query = ReadQuery::with_repo("main", "items");
    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    let cursor_id = expect_first_page(resp);

    quarantine(&handler, "sub", "prof", "main").await;

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    expect_resync_required(resp, "FetchNext after mid-scroll quarantine");
}

/// (c) Cursors over a healthy (actively-subscribed, not quarantined) repo
/// and over a repo with no covering subscription keep working while `main`
/// is quarantined — the gate is per-repo, not per-db.
#[tokio::test]
async fn cursor_reads_healthy_and_non_replicated_repos_unaffected() {
    let handler = build_handler().await;
    let session = alice_session();
    quarantine(&handler, "sub", "prof", "main").await;
    // `side`: replication profile + ACTIVE subscription, never quarantined.
    let mut b = Batch::new();
    b.id(3);
    b.op(
        "profile",
        replication_profile("side_prof").stream(
            repl_scope("app").repo("side").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op(
        "sub",
        subscription("side_sub", "tcp://peer:9000", "pub", "side_prof"),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &b.build())
        .await
        .expect("admin replication batch must succeed");

    let resp = send(
        &handler,
        &session,
        create_cursor_req(ReadQuery::with_repo("side", "items"), 1),
    )
    .await;
    let cursor_id = expect_first_page(resp);
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    assert!(
        matches!(resp, DbResponse::CursorPage { .. }),
        "healthy subscribed repo stays cursor-readable: {resp:?}"
    );

    let resp = send(
        &handler,
        &session,
        create_cursor_req(ReadQuery::with_repo("other", "items"), 1),
    )
    .await;
    assert!(
        matches!(resp, DbResponse::CursorPage { .. }),
        "non-replicated repo stays cursor-readable: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// Point read (`GetRecord`) parked on the held byte budget: LATE quarantine.
// Moved out of the sibling `get_record_tests` module (layout cap). The
// budget-aware builder and the point-read helpers below are test-only copies:
// their originals are module-private to `get_record_tests`.
// ---------------------------------------------------------------------------

/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The byte budget the park test holds for the whole parked window. The
/// handler's `max_result_size_bytes` is deliberately bigger, so its upfront
/// reservation cannot fit while the test holds this slice.
const SMALL_BUDGET: usize = 64;
/// The handler result cap used by the parked fixture (bigger than
/// `SMALL_BUDGET`, bigger than the seeded record's encoded size).
const PARKED_RESULT_CAP: usize = 1024;

/// Limits of the parked fixture: a finite result cap (so the upfront
/// reservation is armed) and the single in-flight slot the park rendezvous
/// observes.
fn parked_limits() -> QueryLimitsCap {
    QueryLimitsCap {
        max_result_size_bytes: PARKED_RESULT_CAP,
        max_in_flight_requests_per_session: 1,
        ..QueryLimitsCap::UNLIMITED
    }
}

/// Handler over an in-memory `ShamirDb` with `app/{main,side}.items`,
/// owned by alice (principal `[0xAB; 16]` — same identity the alice
/// session carries, so alice is the owner). `main` is seeded with one
/// row `{ id: "k000", v: 0 }`; `side` stays empty for the
/// non-replicated-repo assertions. The enforced 0o700 default keeps
/// bob out of everything until a test chmod's levels open.
///
/// With an EXPLICIT byte budget: the park test hands in the budget it holds
/// for the whole parked window, so the point read's upfront reservation
/// (step 6 of `get_record`) cannot fit and the read parks.
async fn build_handler_with_budget(
    byte_budget: ByteBudget,
    query_limits: QueryLimitsCap,
) -> ShamirDbHandler {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    for repo in ["main", "side"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner.clone()))
            .await
            .expect("add repo must complete within the 120s bound")
            .expect("add repo");
    }

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed_k000",
        insert("items").row(doc! { "id" => "k000", "v" => 0_i64 }),
    );
    tokio::time::timeout(CALL_TIMEOUT, shamir.execute_as(owner, "app", &b.build()))
        .await
        .expect("seed row must complete within the 120s bound")
        .expect("seed row");

    ShamirDbHandler::new(Arc::new(shamir))
        .with_cursor_limits(CursorLimitsCap::UNLIMITED)
        .with_query_limits(query_limits)
        .with_byte_budget(byte_budget)
}

/// Insert one row through the ENGINE's real `execute_as` path and return
/// its STORAGE `RecordId` — the typed `QueryRecord::Inserted` /
/// `InsertedRecord::id` the write result carries, never any payload
/// field.
async fn insert_row_and_get_storage_id(handler: &ShamirDbHandler, payload_id: &str) -> RecordId {
    let mut b = Batch::new();
    b.insert(
        "row",
        insert("items").row(doc! { "id" => payload_id, "v" => 1_i64 }),
    );
    let response = tokio::time::timeout(
        CALL_TIMEOUT,
        handler.db().execute_as(Actor::System, "app", &b.build()),
    )
    .await
    .expect("engine insert must complete within the 120s bound")
    .expect("engine insert must succeed");
    let result = response.results.get("row").expect("insert alias returned");
    match result.records.first() {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    }
}

fn expect_error(resp: DbResponse, code: &str, context: &str) {
    match resp {
        DbResponse::Error { code: got, .. } => assert_eq!(got, code, "{context}"),
        other => panic!("{context}: expected error code '{code}', got {other:?}"),
    }
}

fn get_record_req(key: RecordId) -> DbRequest {
    get_record("app", "main", "items", key)
}

/// Drive the point read until it is parked at the upfront budget reservation
/// with the pre-budget resolution (and the pre-park quarantine probe) already
/// done. The session's single request slot is the observable: while we can
/// still take it ourselves the child has not reached step 3, so poll the child
/// once (it must stay Pending, the budget is held); once we cannot take it,
/// the child took it and ran the synchronous step 4/5 code in the same poll,
/// so the capture is done. Sleep/yield free.
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

/// LATE quarantine: the step-4 probe runs BEFORE the park, so only the
/// acquisition-time gate (step 8 — `ShamirDb::get_table` re-checks the
/// repository quarantine) can see a quarantine that lands while the read is
/// parked. Both sibling tests above quarantine BEFORE the read, so this is
/// the seam they cannot cover.
///
/// The row is seeded and read back while the repo is HEALTHY (CONTROL), the
/// point read then parks on the held budget, `quarantine(...)` flips the repo
/// to `resync_required`, and the budget is released: the released read must
/// answer the FIXED point-read text — never the record, never the stored
/// reason.
///
/// MUTANT "the quarantine is consulted only at the pre-park probe" (the
/// acquisition-time gate dropped): the parked read resolves the table and
/// returns the seeded record, so this test fails on the payload. RED on the
/// parent too, which has no test at all that quarantines after the park.
#[tokio::test]
async fn get_record_refuses_when_the_repo_is_quarantined_while_parked() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let handler = build_handler_with_budget(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    // Stamp through the store: only a store-minted session carries the
    // nonzero activity handle the park rendezvous observes.
    let session = store.insert(ALICE_SID, alice_session());
    assert_ne!(session.activity_handle(), 0, "store-stamped session");

    // Seed AND read the row while the repository is still HEALTHY. The
    // control handler shares the db but owns no budget, so it cannot park.
    let storage_id = insert_row_and_get_storage_id(&handler, "k008").await;
    let control_handler =
        ShamirDbHandler::new(Arc::clone(handler.db())).with_byte_budget(ByteBudget::unbounded());
    let control = tokio::time::timeout(
        CALL_TIMEOUT,
        send(&control_handler, &session, get_record_req(storage_id)),
    )
    .await
    .expect("the control read must complete within the 120s bound");
    assert!(
        matches!(control, DbResponse::Record { .. }),
        "CONTROL: the seeded row must read back while the repo is healthy: {control:?}"
    );

    // Park the SAME read on the held budget.
    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(&handler, &session, get_record_req(storage_id)));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    // LATE quarantine: strictly after the pre-park probe AND after the park.
    tokio::time::timeout(CALL_TIMEOUT, quarantine(&handler, "sub", "prof", "main"))
        .await
        .expect("the late quarantine must complete within the 120s bound");
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    // The code through the file's shared helper (which checks the code only
    // and drops the message), the FIXED text asserted exactly below.
    let message = match &resp {
        DbResponse::Error { message, .. } => message.clone(),
        other => panic!("expected the fixed resync_required refusal, got {other:?}"),
    };
    expect_error(resp, "resync_required", "quarantined while parked");
    assert_eq!(
        message, "repository is quarantined; resynchronize before reading",
        "the parked refusal must be the fixed point-read text"
    );
    assert!(
        !message.contains("journal gap"),
        "the stored quarantine reason must never be echoed verbatim, got {message:?}"
    );
}
