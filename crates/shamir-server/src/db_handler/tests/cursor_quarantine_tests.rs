//! S5c-X PR8b — cursor reads (`CreateCursor`/`FetchNext`) are gated behind
//! the `resync_required` read-quarantine: a cursor must not be created over
//! a quarantined repo, a cursor created BEFORE the quarantine must fail
//! closed on the next fetch, and healthy / non-replicated repos are
//! unaffected. Fixture mirrors `cursor_handler_tests.rs` (its helpers are
//! module-private, so the minimal setup is duplicated here re-export-free).

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
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_query_types::read::ReadQuery;
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use shamir_query_builder::doc;

use crate::db_handler::config::CursorLimitsCap;
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
    shamir.create_db_as("app", owner.clone()).await;
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
                insert::with_repo(repo, "items").row(doc! { "id" => k, "v" => v }),
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
