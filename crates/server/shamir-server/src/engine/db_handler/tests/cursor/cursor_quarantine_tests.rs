//! S5c-X PR8b — cursor reads (`CreateCursor`/`FetchNext`) are gated behind
//! the `resync_required` read-quarantine: a cursor must not be created over
//! a quarantined repo, a cursor created BEFORE the quarantine must fail
//! closed on the next fetch, and healthy / non-replicated repos are
//! unaffected. Fixture mirrors `cursor_handler_tests.rs` (its helpers are
//! module-private, so the minimal setup is duplicated here re-export-free).

use std::sync::Arc;
use std::time::Duration;

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
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::{insert, Insert};
use shamir_query_types::admin::{ReplDirection, ReplMode, ResourceRef};
use shamir_query_types::read::{QueryRecord, ReadQuery};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use shamir_types::types::record_id::RecordId;

use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap};
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const ALICE_SID: [u8; 32] = [0xAA; 32];
const BOB_SID: [u8; 32] = [0xBB; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The never-created table name used as the absent case in every pair.
const ABSENT_TABLE: &str = "absent_items";
/// The single fixed refusal text an absent table and a closed table share.
const FIXED_REFUSAL: &str = "table is unavailable";

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
// READ-refusal uniformity across the `GetRecord`, batch-read and cursor-create
// paths (moved here verbatim from the former `read_refusal_uniformity_tests`
// module, which the layout folder limit no longer allows as its own file).
// The natural sibling `get_record_tests.rs` is already at the 1000-line file
// limit, so the block lives in this folder's other refusal-gating module.
// ---------------------------------------------------------------------------

// An actor that cannot READ a table must not be able to tell whether that
// table EXISTS.
//
// Before this rule, the table ACL was consulted on the raw NAME: a closed
// table answered `access_denied`, while an absent name passed the
// (default-open) table check and was then refused for absence — `query` on
// the batch path, `not_found`-ish legacy codes on the cursor path. That
// difference is an existence oracle.
//
// Each test below asks the SAME actor the SAME question twice — once about
// an existing table it may not read, once about an absent name — and
// asserts the two wire codes are EQUAL (and equal to `unknown_table`). The
// equality is the invariant; the literal pins which code the invariant
// resolved to.
//
// Fixture mirrors `get_record_tests.rs` (in-memory `ShamirDb`, owner alice,
// repo `main` with table `items`, one seeded row; requests built through
// `shamir_query_builder` and dispatched through the real handler; every
// await is bounded by a 120 s timeout because the Windows CI runner may be
// up to 10x slower). No temp dirs and no sleeps: the repository is purely
// in-memory and lives for the whole test.

/// Unrelated non-owner, non-superuser actor.
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

/// Handler over an in-memory `ShamirDb` with `app.main.items` owned by alice
/// (principal `[0xAB; 16]`), plus one seeded row. `items` therefore EXISTS
/// and is closed to bob until a test opens the ancestors.
///
/// Distinct from this module's `build_handler` (which builds three repos with
/// three rows each): the owner control below asserts EXACTLY one row comes
/// back, so the uniformity fixture keeps its own single-row seeding.
async fn build_uniformity_handler() -> ShamirDbHandler {
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
    tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner.clone()))
        .await
        .expect("add repo must complete within the 120s bound")
        .expect("add repo");

    let handler = ShamirDbHandler::new(Arc::new(shamir))
        .with_cursor_limits(CursorLimitsCap::UNLIMITED)
        .with_query_limits(QueryLimitsCap::UNLIMITED);

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed",
        insert("items").row(doc! { "id" => "k000", "v" => 0_i64 }),
    );
    tokio::time::timeout(
        CALL_TIMEOUT,
        handler.db().execute_as(owner, "app", &b.build()),
    )
    .await
    .expect("engine insert must complete within the 120s bound")
    .expect("engine insert must succeed");
    handler
}

/// Open ONLY the database and store ancestors to `mode`, leaving `items` at
/// its enforced 0o700 default — the "Database-read
/// but Table-read-denied" ACL state (mirrors
/// `get_record_tests::open_db_and_store_only`).
async fn open_db_and_store_only(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "db",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Database {
                database: "app".into(),
            },
            mode,
        )
        .hmac("read-refusal-uniformity-test")
        .build()
        .expect("build chmod db"),
    );
    batch.chmod(
        "store",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Store {
                store: ["app".into(), "main".into()],
            },
            mode,
        )
        .hmac("read-refusal-uniformity-test")
        .build()
        .expect("build chmod store"),
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

/// Send a wire request through the REAL `RequestHandler::handle`, bounded by
/// `CALL_TIMEOUT`. This module's bare `send` carries no hang detector, so the
/// merged tests keep their own bounded variant instead of silently losing
/// the 120 s bound.
async fn send_with_call_timeout(
    handler: &ShamirDbHandler,
    session: &Session,
    req: DbRequest,
) -> DbResponse {
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

/// Extract the WHOLE wire error (code AND message), panicking on a
/// non-error response. Comparing the pair is what makes the refusal parity
/// whole-response, not code-only.
fn error_pair_of(resp: DbResponse, context: &str) -> (String, String) {
    match resp {
        DbResponse::Error { code, message } => (code, message),
        other => panic!("{context}: expected an error, got {other:?}"),
    }
}

fn batch_read_req(table: &str) -> DbRequest {
    let mut batch = Batch::new();
    batch.id(1);
    batch.query("read", Query::with_repo("main", table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch: batch.build(),
    }
}

/// A quarantined repository must not bypass READ availability preflight:
/// for the same database-READ-authorized session, an absent table and an
/// existing table with Table Read closed both produce the fixed table
/// refusal before the quarantine gate can disclose the repository state.
#[tokio::test]
async fn quarantined_get_record_refusal_is_identical_for_closed_and_absent_tables() {
    let handler = build_uniformity_handler().await;
    // Preserve Database Read (and Store access); only the table's default
    // 0o700 ACL remains closed to bob.
    open_db_and_store_only(&handler, 0o777).await;
    quarantine(&handler, "sub", "prof", "main").await;
    let bob = bob_session();
    let key = RecordId([1u8; 16]);

    let closed = error_pair_of(
        send_with_call_timeout(&handler, &bob, get_record("app", "main", "items", key)).await,
        "GetRecord on the closed table in a quarantined repo",
    );
    let absent = error_pair_of(
        send_with_call_timeout(&handler, &bob, get_record("app", "main", ABSENT_TABLE, key)).await,
        "GetRecord on the absent table in a quarantined repo",
    );

    assert_eq!(
        closed, absent,
        "quarantine must not change READ availability parity"
    );
    assert_eq!(
        closed,
        ("unknown_table".to_string(), FIXED_REFUSAL.to_string())
    );

    // Positive control: the table exists and the repository quarantine is
    // active; a reader with Table Read reaches the quarantine refusal.
    open_db_and_store_only(&handler, 0o777).await;
    chmod_items_table_to(&handler, 0o777).await;
    let quarantined = error_pair_of(
        send_with_call_timeout(
            &handler,
            &alice_session(),
            get_record("app", "main", "items", key),
        )
        .await,
        "GetRecord on an open table in a quarantined repo",
    );
    assert_eq!(quarantined.0, "resync_required");
}

/// `GetRecord`: a closed table and an absent name must answer the SAME
/// code. WITHOUT the rule the closed table answers `access_denied` while
/// the absent name answers `unknown_table`, so both the equality and the
/// literal below fail.
#[tokio::test]
async fn get_record_refusal_is_identical_for_closed_and_absent_tables() {
    let handler = build_uniformity_handler().await;
    open_db_and_store_only(&handler, 0o777).await;
    let bob = bob_session();
    let key = RecordId([1u8; 16]);

    let closed = error_pair_of(
        send_with_call_timeout(&handler, &bob, get_record("app", "main", "items", key)).await,
        "GetRecord on the closed table",
    );
    let absent = error_pair_of(
        send_with_call_timeout(&handler, &bob, get_record("app", "main", ABSENT_TABLE, key)).await,
        "GetRecord on the absent table",
    );

    assert_eq!(
        closed, absent,
        "GetRecord must not reveal whether the table exists"
    );
    assert_eq!(closed.0, "unknown_table");
    assert_eq!(closed.1, FIXED_REFUSAL);
}

/// Batch read: a closed table and an absent name must answer the SAME code.
/// WITHOUT the rule the closed table is denied by `Authorized::authorize`
/// (`access_denied`) and the absent name fails `validate_tables` (the
/// legacy-mapped generic `query`), so the equality fails.
#[tokio::test]
async fn batch_read_refusal_is_identical_for_closed_and_absent_tables() {
    let handler = build_uniformity_handler().await;
    open_db_and_store_only(&handler, 0o777).await;
    let bob = bob_session();

    let closed = error_pair_of(
        send_with_call_timeout(&handler, &bob, batch_read_req("items")).await,
        "batch read of the closed table",
    );
    let absent = error_pair_of(
        send_with_call_timeout(&handler, &bob, batch_read_req(ABSENT_TABLE)).await,
        "batch read of the absent table",
    );

    assert_eq!(
        closed, absent,
        "a batch read must not reveal whether the table exists"
    );
    assert_eq!(closed.0, "unknown_table");
    // The whole message must be identical for both cases and carry the fixed
    // refusal text (the batch path may wrap it in its own error envelope).
    assert!(
        closed.1.ends_with(FIXED_REFUSAL),
        "batch read must carry the fixed refusal text, got {:?}",
        closed.1
    );
}

/// Cursor create with invalid pagination: a closed table and an absent name
/// must answer the same fixed refusal before pagination validation can reveal
/// that the request reached target-dependent checks.
#[tokio::test]
async fn cursor_create_invalid_page_size_refusal_precedes_validation_for_closed_and_absent() {
    let handler = build_uniformity_handler().await;
    open_db_and_store_only(&handler, 0o777).await;
    let bob = bob_session();

    let closed = error_pair_of(
        send_with_call_timeout(
            &handler,
            &bob,
            create_cursor_req(ReadQuery::with_repo("main", "items"), 0),
        )
        .await,
        "cursor create on the closed table",
    );
    let absent = error_pair_of(
        send_with_call_timeout(
            &handler,
            &bob,
            create_cursor_req(ReadQuery::with_repo("main", ABSENT_TABLE), 0),
        )
        .await,
        "cursor create on the absent table",
    );

    assert_eq!(
        closed, absent,
        "cursor create must not reveal whether the table exists"
    );
    assert_eq!(closed.0, "unknown_table");
    // `create_cursor` maps the shared `BatchError` through `error_response`,
    // so the fixed text rides inside that envelope; the whole message must
    // still be identical for both cases and end with the fixed refusal.
    assert!(
        closed.1.ends_with(FIXED_REFUSAL),
        "cursor create must carry the fixed refusal text, got {:?}",
        closed.1
    );
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a refused cursor create must not register a cursor"
    );
}

/// Give the uniformity fixture enough rows for a page_size-1 cursor to keep
/// a live cursor (its first page must report `has_more`).
async fn seed_extra_items(handler: &ShamirDbHandler) {
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "extra0",
        insert("items").row(doc! { "id" => "k001", "v" => 1_i64 }),
    );
    b.insert(
        "extra1",
        insert("items").row(doc! { "id" => "k002", "v" => 2_i64 }),
    );
    tokio::time::timeout(
        CALL_TIMEOUT,
        handler
            .db()
            .execute_as(Actor::User(principal64([0xAB; 16])), "app", &b.build()),
    )
    .await
    .expect("seed must complete within the 120s bound")
    .expect("seed must succeed");
}

/// Chmod `app.main.items` to `mode` as the System actor.
async fn chmod_items_table_to(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "table",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        )
        .hmac("cursor-quarantine-fetch-refusal-test")
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
    .expect("chmod must complete within the 120s bound")
    .expect("chmod must succeed");
}

/// Drop `app.main.items` through the engine's admission-checked path.
async fn drop_items_table(handler: &ShamirDbHandler) {
    let db = handler.db().get_db("app").expect("app database");
    let removed = tokio::time::timeout(CALL_TIMEOUT, db.drop_table("main", "items"))
        .await
        .expect("drop must complete within the 120s bound")
        .expect("drop must succeed");
    assert!(removed, "the fixture table must actually be dropped");
}

/// Fetch-lifecycle PAIR: a Read revoked before `FetchNext` and the table
/// dropped before `FetchNext` must answer the SAME fixed refusal, BOTH must
/// leave the cursor registry empty, and an explicit close afterwards must
/// stay idempotent (`CursorClosed`).
#[tokio::test]
async fn fetch_refusal_is_identical_for_revoked_and_dropped_tables_and_retires_cursor() {
    let handler = build_uniformity_handler().await;
    seed_extra_items(&handler).await;
    let alice = alice_session();

    let revoked_cursor = expect_first_page(
        send_with_call_timeout(
            &handler,
            &alice,
            create_cursor_req(ReadQuery::with_repo("main", "items"), 1),
        )
        .await,
    );
    chmod_items_table_to(&handler, 0o000).await;
    let revoked = send_with_call_timeout(&handler, &alice, fetch_next_req(revoked_cursor, 1)).await;
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a revoked Read on FetchNext must retire the cursor"
    );
    let closed = send_with_call_timeout(
        &handler,
        &alice,
        DbRequest::CancelCursor {
            cursor_id: revoked_cursor,
        },
    )
    .await;
    assert!(
        matches!(closed, DbResponse::CursorClosed { cursor_id } if cursor_id == revoked_cursor),
        "an explicit close after the refusal must stay idempotent"
    );

    chmod_items_table_to(&handler, 0o777).await;
    let dropped_cursor = expect_first_page(
        send_with_call_timeout(
            &handler,
            &alice,
            create_cursor_req(ReadQuery::with_repo("main", "items"), 1),
        )
        .await,
    );
    drop_items_table(&handler).await;
    let dropped = send_with_call_timeout(&handler, &alice, fetch_next_req(dropped_cursor, 1)).await;
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a table miss on FetchNext must retire the cursor"
    );
    let closed = send_with_call_timeout(
        &handler,
        &alice,
        DbRequest::CancelCursor {
            cursor_id: dropped_cursor,
        },
    )
    .await;
    assert!(
        matches!(closed, DbResponse::CursorClosed { cursor_id } if cursor_id == dropped_cursor),
        "an explicit close after the table miss must stay idempotent"
    );

    let revoked = error_pair_of(revoked, "FetchNext after a revoked Read");
    let dropped = error_pair_of(dropped, "FetchNext after the table is dropped");
    assert_eq!(
        revoked, dropped,
        "a revoked Read and a table miss must be the same refusal"
    );
    assert_eq!(revoked.0, "unknown_table");
    assert_eq!(revoked.1, FIXED_REFUSAL);
}

/// Control: the seed row actually EXISTS and the closed table really is the
/// one bob may not read — a positive read by the owner returns it, so the
/// three equality tests above cannot pass vacuously on a missing fixture
/// table.
#[tokio::test]
async fn owner_control_reads_the_seeded_row() {
    let handler = build_uniformity_handler().await;
    let alice = alice_session();

    let resp = send_with_call_timeout(&handler, &alice, batch_read_req("items")).await;
    match resp {
        DbResponse::Batch { response } => {
            let result = response.results.get("read").expect("read alias returned");
            assert_eq!(result.records.len(), 1, "the seeded row must be readable");
            assert!(matches!(result.records[0], QueryRecord::Direct(_)));
        }
        other => panic!("expected the owner's Batch response, got {other:?}"),
    }
}
