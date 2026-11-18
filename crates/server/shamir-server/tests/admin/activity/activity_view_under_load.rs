//! `activity_view_under_load` (R4b-1, design §6, narrowed): exactly 50 live
//! stored sessions (5 fixture users x 10, under the per-user cap of 16),
//! each holding one open interactive transaction and one open, un-drained
//! cursor (4 seeded committed rows, page size 1), while the admin pages
//! `ActivityKind::All` through `scope: All` to exhaustion with a limit of
//! 16. The observed id sets are compared EXACTLY against the recorded
//! fixture sets, per row variant; `Requests` must be empty in this
//! quiescent test (the running-request observations live in the privacy
//! test). After typed teardown (`TxRollback` + `CancelCursor` per fixture,
//! sessions removed from the store) the final `All` page is empty.
//!
//! The `Own`-scope filtering is proven by `permission_denial_activity` and
//! is not repeated here. The whole test body is bounded by one Tokio
//! timeout and carries a `FlakeGuard` watchdog.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::session::{Session, SessionStore};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRow, ActivityScope, ActivityTxState, ListActivityReq,
};
use shamir_query_types::wire::CursorId;
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use shamir_test_diag::FlakeGuard;

use super::common::{
    call, create_cursor_req, list_activity_req, make_handler, make_session, on_big_stack,
    open_engine, register_session, seed_committed_rows, tx_begin_req, tx_insert_req, DB, TABLE,
};

/// Fixture users: 5 users x 10 sessions = 50, under the per-user cap of 16.
const LOAD_USERS: [&str; 5] = ["act_u0", "act_u1", "act_u2", "act_u3", "act_u4"];
const SESSIONS_PER_USER: usize = 10;
const PAGE_LIMIT: u32 = 16;
/// 150 live rows (50 sessions + 50 txs + 50 cursors) at limit 16 exhaust in
/// 10 pages; the bound only proves exhaustion happened.
const MAX_PAGES: usize = 16;
/// Seeded committed rows so every fixture cursor is un-drained (page_size
/// 1 over 4 rows still has more pages to hand out).
const SEEDED_ROWS: i64 = 4;
/// Hard bound for the WHOLE test body (every await inside is separately
/// bounded by the fixture's per-call timeout).
const WHOLE_BODY_TIMEOUT: Duration = Duration::from_secs(300);

/// One live fixture: the published store generation (keeps the session
/// live), its opened transaction handle (`u64`, as on the wire) and its
/// opened cursor id (the actual wire newtype from `DbResponse::CursorPage`).
struct Fixture {
    session: Arc<Session>,
    tx_handle: u64,
    cursor_id: CursorId,
}

/// Page `kind` to exhaustion with `limit` rows per page (each call is a
/// bounded fixture call); returns every rendered row.
async fn walk(
    handler: &ShamirDbHandler,
    admin: &Session,
    kind: ActivityKind,
    scope: ActivityScope,
    limit: Option<u32>,
) -> Vec<ActivityRow> {
    let mut rows = Vec::new();
    let mut continuation: Option<String> = None;
    for _page in 0..MAX_PAGES {
        let req = ListActivityReq {
            scope,
            kind,
            limit,
            continuation: continuation.clone(),
        };
        let resp = call(handler, admin, &list_activity_req(req)).await;
        let page = match resp {
            DbResponse::ActivityPage { page } => page,
            other => panic!("expected ActivityPage, got {other:?}"),
        };
        if let Some(bound) = limit {
            assert!(
                page.rows.len() <= bound as usize,
                "page bound violated: {} rows > {bound}",
                page.rows.len()
            );
        }
        rows.extend(page.rows);
        continuation = page.continuation;
        if continuation.is_none() {
            return rows;
        }
    }
    panic!("continuation walk did not exhaust within {MAX_PAGES} pages");
}

/// Fold an `All` walk into per-variant id sets, checking every row-shape
/// invariant on the way. `Requests` must not appear (quiescent test).
fn fold_all_rows(
    rows: Vec<ActivityRow>,
    sessions: &mut BTreeSet<u64>,
    txs: &mut BTreeSet<u64>,
    cursors: &mut BTreeSet<u64>,
    session_handles: &BTreeSet<u64>,
) {
    for row in rows {
        match row {
            ActivityRow::Session {
                session_handle,
                open_transactions,
                open_cursors,
                ..
            } => {
                assert!(
                    sessions.insert(session_handle),
                    "duplicate session handle {session_handle}"
                );
                assert_eq!(
                    (open_transactions, open_cursors),
                    (1, 1),
                    "fixture session must hold exactly one tx and one cursor"
                );
            }
            ActivityRow::Transaction {
                tx_handle,
                session_handle,
                state,
                ..
            } => {
                assert!(txs.insert(tx_handle), "duplicate tx handle {tx_handle}");
                assert!(
                    session_handles.contains(&session_handle),
                    "tx references unknown session handle {session_handle}"
                );
                assert!(
                    matches!(
                        state,
                        ActivityTxState::Active | ActivityTxState::AwaitingCommit
                    ),
                    "live fixture tx must not be expired-pending-reap"
                );
            }
            ActivityRow::Cursor {
                cursor_id,
                session_handle,
                table,
                ..
            } => {
                assert!(cursors.insert(cursor_id), "duplicate cursor id {cursor_id}");
                assert!(
                    session_handles.contains(&session_handle),
                    "cursor references unknown session handle {session_handle}"
                );
                assert_eq!(table, TABLE, "cursor rows carry the table NAME only");
            }
            ActivityRow::Request { .. } => {
                panic!("no request rows may exist in this quiescent test")
            }
        }
    }
}

#[test]
fn activity_view_under_load() {
    let guard = FlakeGuard::with_watchdog("activity_view_under_load", WHOLE_BODY_TIMEOUT);
    on_big_stack(move || async move {
        tokio::time::timeout(WHOLE_BODY_TIMEOUT, run(guard))
            .await
            .expect("activity_view_under_load within the whole-body timeout");
    });
}

async fn run(guard: FlakeGuard) {
    let shamir = open_engine().await;
    let store = Arc::new(SessionStore::new());
    let handler = make_handler(&shamir, Arc::clone(&store));

    // The paging admin is a DIRECT session (not registered in the store):
    // authorization uses the passed `&Session`, and the view reads its rows
    // from the registries — an unregistered admin contributes no rows.
    let admin = make_session("admin", 0xFF, true);

    guard.mark("seed");
    // Seed committed data BEFORE opening cursors so cursors are live and
    // un-drained; asserted directly on each cursor-open response below.
    seed_committed_rows(&handler, &admin, SEEDED_ROWS).await;

    guard.mark("open-sessions");
    let mut fixtures: Vec<Fixture> = Vec::new();
    let mut expected_sessions: BTreeSet<u64> = BTreeSet::new();
    let mut expected_txs: BTreeSet<u64> = BTreeSet::new();
    let mut expected_cursors: BTreeSet<u64> = BTreeSet::new();
    for (u, user) in LOAD_USERS.iter().enumerate() {
        for i in 0..SESSIONS_PER_USER {
            let session = register_session(&store, make_session(user, (u * 20 + i) as u8, false));
            expected_sessions.insert(session.activity_handle());
            let tx_handle = match call(&handler, &session, &tx_begin_req()).await {
                DbResponse::TxOpened { tx_handle, .. } => tx_handle,
                other => panic!("tx begin for {user}: {other:?}"),
            };
            expected_txs.insert(tx_handle);
            // Unique staged value per session — no shared-row contention.
            let staged = 1_000 + (u * SESSIONS_PER_USER + i) as i64;
            assert!(matches!(
                call(&handler, &session, &tx_insert_req(tx_handle, staged)).await,
                DbResponse::TxBatch { .. }
            ));
            let cursor_id = match call(&handler, &session, &create_cursor_req()).await {
                DbResponse::CursorPage {
                    cursor_id,
                    has_more: true,
                    ..
                } => cursor_id,
                DbResponse::CursorPage { has_more, .. } => panic!(
                    "cursor must be un-drained (seeded {SEEDED_ROWS} rows, page_size 1); got has_more={has_more}"
                ),
                other => panic!("cursor open for {user}: {other:?}"),
            };
            expected_cursors.insert(cursor_id.0);
            fixtures.push(Fixture {
                session,
                tx_handle,
                cursor_id,
            });
        }
    }
    assert_eq!(fixtures.len(), 50, "exactly 50 live stored sessions");
    guard.note("sessions", fixtures.len().to_string());

    guard.mark("walk-all-to-exhaustion");
    let all_rows = walk(
        &handler,
        &admin,
        ActivityKind::All,
        ActivityScope::All,
        Some(PAGE_LIMIT),
    )
    .await;
    assert_eq!(
        all_rows.len(),
        fixtures.len() * 3,
        "the All walk must render every fixture row exactly once"
    );
    let mut observed_sessions: BTreeSet<u64> = BTreeSet::new();
    let mut observed_txs: BTreeSet<u64> = BTreeSet::new();
    let mut observed_cursors: BTreeSet<u64> = BTreeSet::new();
    fold_all_rows(
        all_rows,
        &mut observed_sessions,
        &mut observed_txs,
        &mut observed_cursors,
        &expected_sessions,
    );
    assert_eq!(
        observed_sessions, expected_sessions,
        "observed session handles must equal the recorded fixture set"
    );
    assert_eq!(
        observed_txs, expected_txs,
        "observed tx handles must equal the recorded fixture set"
    );
    assert_eq!(
        observed_cursors, expected_cursors,
        "observed cursor ids (via .0) must equal the recorded fixture set"
    );

    guard.mark("requests-empty");
    let requests = walk(
        &handler,
        &admin,
        ActivityKind::Requests,
        ActivityScope::All,
        Some(PAGE_LIMIT),
    )
    .await;
    assert!(
        requests.is_empty(),
        "a quiescent test must list no in-flight requests"
    );

    guard.mark("negative-controls");
    // `limit: 0` is rejected as typed `invalid_request` (D4), never clamped.
    let zero = call(
        &handler,
        &admin,
        &list_activity_req(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::All,
            limit: Some(0),
            continuation: None,
        }),
    )
    .await;
    assert!(
        matches!(zero, DbResponse::Error { ref code, .. } if code == "invalid_request"),
        "limit 0 must be invalid_request: {zero:?}"
    );
    // A limit above the 256 cap is CLAMPED, not rejected (D4): the whole
    // 150-row view fits one clamped page with no continuation.
    let clamped = walk(
        &handler,
        &admin,
        ActivityKind::All,
        ActivityScope::All,
        Some(u32::MAX),
    )
    .await;
    assert_eq!(
        clamped.len(),
        fixtures.len() * 3,
        "a clamped oversized limit must render every row in one call"
    );

    guard.mark("hard-cap-witness");
    // 150 rows cannot distinguish "no cap" from "cap = 256". Store 257 more
    // distinct sessions (fixed non-secret sids: a namespace byte distinct
    // from every fixture tag plus the u16 index in the next two bytes) so
    // the cap MUST truncate. With an oversize `limit` the compact
    // `Sessions` view must return EXACTLY 256 rows plus a continuation —
    // with no cap in place this would return 307 rows in one page.
    const CAP_WITNESS: usize = 257;
    const CAP_NS: u8 = 0xA0;
    let mut witness_sids: Vec<[u8; 32]> = Vec::with_capacity(CAP_WITNESS);
    for i in 0..CAP_WITNESS {
        let mut sid = [0u8; 32];
        sid[0] = CAP_NS;
        sid[1..3].copy_from_slice(&(i as u16).to_be_bytes());
        // Unique usernames keep every session under the per-user cap.
        let mut session = make_session(&format!("cap_witness_{i}"), 0, false);
        session.session_id = sid;
        register_session(&store, session);
        witness_sids.push(sid);
    }
    let witness_resp = call(
        &handler,
        &admin,
        &list_activity_req(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::Sessions,
            limit: Some(u32::MAX),
            continuation: None,
        }),
    )
    .await;
    let witness_page = match witness_resp {
        DbResponse::ActivityPage { page } => page,
        other => panic!("expected ActivityPage, got {other:?}"),
    };
    assert_eq!(
        witness_page.rows.len(),
        256,
        "oversize limit must clamp to the 256 hard cap (witness rows + 50 fixtures = 307 sessions)"
    );
    assert!(
        witness_page
            .rows
            .iter()
            .all(|row| matches!(row, ActivityRow::Session { .. })),
        "the Sessions view must render only session rows"
    );
    assert!(
        witness_page.continuation.is_some(),
        "a capped page must hand off a continuation past its last key"
    );
    // Clean ONLY the witness fixtures (fixed sids, namespace 0xA0); the
    // 50-session load fixtures keep their typed teardown below.
    for sid in &witness_sids {
        assert!(store.remove(sid).is_some(), "witness session removed");
    }
    drop(witness_sids);

    guard.mark("teardown");
    // Typed teardown: roll back every fixture tx and cancel every fixture
    // cursor through the wire requests, checking the typed replies.
    for fixture in &fixtures {
        match call(
            &handler,
            &fixture.session,
            &DbRequest::TxRollback {
                db: DB.into(),
                tx_handle: fixture.tx_handle,
            },
        )
        .await
        {
            DbResponse::TxRolledBack { tx_handle } => {
                assert_eq!(tx_handle, fixture.tx_handle, "rolled-back handle echoed")
            }
            other => panic!("tx rollback for {}: {other:?}", fixture.tx_handle),
        }
        match call(
            &handler,
            &fixture.session,
            &DbRequest::CancelCursor {
                cursor_id: fixture.cursor_id,
            },
        )
        .await
        {
            DbResponse::CursorClosed { cursor_id } => {
                assert_eq!(cursor_id, fixture.cursor_id, "closed cursor id echoed")
            }
            other => panic!("cancel cursor for {}: {other:?}", fixture.cursor_id),
        }
    }
    // Remove every fixture session from the store; the view must drain to
    // empty immediately (no polls, no sleeps — teardown is deterministic).
    for fixture in &fixtures {
        store.remove(&fixture.session.session_id);
    }
    drop(fixtures);

    let drained = walk(
        &handler,
        &admin,
        ActivityKind::All,
        ActivityScope::All,
        Some(PAGE_LIMIT),
    )
    .await;
    assert!(
        drained.is_empty(),
        "after teardown the All page must be empty, got {} rows",
        drained.len()
    );

    guard.mark("done");
}
