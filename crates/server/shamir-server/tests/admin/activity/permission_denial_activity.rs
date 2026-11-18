//! `permission_denial_activity` (R4b-1, design §6): the default permission
//! posture of the activity view, driven through the public `handle` wire
//! entry with typed fixture sessions.
//!
//! - An ordinary user asking `scope: All` is denied (`permission_denied`).
//! - The same ordinary user asking `scope: Own` is allowed and sees only
//!   their own rows — across multiple sessions of the same user — and
//!   never another user's rows or the superuser's.
//!
//! The follower-side behaviour (ReadOnly node allows the read-only
//! listing per role) lives in the private handler test
//! `src/engine/db_handler/tests/policy/activity_follower_view_tests.rs`.

use std::time::Duration;

use shamir_connect::server::session::SessionStore;
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRow, ActivityScope, ListActivityReq,
};
use shamir_server::db_handler::DbResponse;
use shamir_test_diag::FlakeGuard;

use super::common::{
    call, create_cursor_req, list_activity_req, make_handler, make_session, on_big_stack,
    open_engine, register_session, seed_committed_rows, tx_begin_req, tx_insert_req,
};

const ALICE: &str = "act_alice";

fn own_all() -> ListActivityReq {
    ListActivityReq {
        scope: ActivityScope::Own,
        kind: ActivityKind::All,
        limit: None,
        continuation: None,
    }
}

fn all_all() -> ListActivityReq {
    ListActivityReq {
        scope: ActivityScope::All,
        kind: ActivityKind::All,
        limit: None,
        continuation: None,
    }
}

fn row_user(row: &ActivityRow) -> &str {
    match row {
        ActivityRow::Session { user, .. }
        | ActivityRow::Request { user, .. }
        | ActivityRow::Transaction { user, .. }
        | ActivityRow::Cursor { user, .. } => user,
    }
}

#[test]
fn permission_denial_activity() {
    let guard = FlakeGuard::with_watchdog("permission_denial_activity", Duration::from_secs(120));
    on_big_stack(move || async move {
        run(guard).await;
    });
}

async fn run(guard: FlakeGuard) {
    let shamir = open_engine().await;
    let store = std::sync::Arc::new(SessionStore::new());
    let handler = make_handler(&shamir, std::sync::Arc::clone(&store));

    let admin = register_session(&store, make_session("admin", 0xFE, true));

    guard.mark("alice-sessions");
    // Two sessions of the same ordinary user; the first holds a live tx
    // and an un-drained cursor so the Own scope has real rows to expose.
    // The cursor hands out COMMITTED rows, so seed two committed rows
    // first (the page below positively asserts a follow-up page exists).
    let alice1 = register_session(&store, make_session(ALICE, 0xA1, false));
    let alice2 = register_session(&store, make_session(ALICE, 0xA2, false));
    seed_committed_rows(&handler, &admin, 2).await;
    let tx_handle = match call(&handler, &alice1, &tx_begin_req()).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("alice tx begin: {other:?}"),
    };
    assert!(matches!(
        call(&handler, &alice1, &tx_insert_req(tx_handle, 1)).await,
        DbResponse::TxBatch { .. }
    ));
    assert!(
        matches!(
            call(&handler, &alice1, &create_cursor_req()).await,
            DbResponse::CursorPage { has_more: true, .. }
        ),
        "seeded committed rows must leave the cursor a follow-up page"
    );

    guard.mark("all-denied");
    // (a) Default denial: ordinary user, scope All, from BOTH alice
    // sessions — no ActivityView grant exists for either.
    for (label, session) in [("alice1", &alice1), ("alice2", &alice2)] {
        let resp = call(&handler, session, &list_activity_req(all_all())).await;
        assert!(
            matches!(resp, DbResponse::Error { ref code, .. } if code == "permission_denied"),
            "{label}: scope All without the ActivityView grant must be permission_denied"
        );
    }

    guard.mark("own-filtered");
    // (b) The superuser's `All` listing shows alice's rows — and alice's
    // `Own` listing shows ONLY alice's rows, never the admin's.
    let admin_view = match call(&handler, &admin, &list_activity_req(all_all())).await {
        DbResponse::ActivityPage { page } => page,
        other => panic!("admin scope All: {other:?}"),
    };
    assert!(
        admin_view
            .rows
            .iter()
            .any(|r| matches!(r, ActivityRow::Transaction { user, .. } if user == ALICE)),
        "admin All view must contain alice's live transaction"
    );

    let alice_view = match call(&handler, &alice2, &list_activity_req(own_all())).await {
        DbResponse::ActivityPage { page } => page,
        other => panic!("alice scope Own: {other:?}"),
    };
    assert!(
        !alice_view.rows.is_empty(),
        "alice's Own view must contain her own live rows"
    );
    for row in &alice_view.rows {
        assert_eq!(row_user(row), ALICE, "Own scope leaked a foreign row");
    }
    assert!(
        alice_view
            .rows
            .iter()
            .any(|r| matches!(r, ActivityRow::Transaction { .. })),
        "alice's Own view must contain her own open transaction (same user, other session)"
    );
    assert!(
        !alice_view
            .rows
            .iter()
            .any(|r| matches!(r, ActivityRow::Session { user, .. } if user == "admin")),
        "Own scope must not contain the admin session"
    );

    guard.mark("done");
}
