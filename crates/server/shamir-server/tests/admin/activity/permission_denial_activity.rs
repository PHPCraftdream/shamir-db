//! `permission_denial_activity` (R4b-1, design §6): the default permission
//! posture of the activity view, driven through the public `handle` wire
//! entry with typed fixture sessions, on a persistent (Fjall) catalogue.
//!
//! - An ordinary user asking `scope: All` is denied (`permission_denied`).
//! - The same ordinary user asking `scope: Own` is allowed and sees only
//!   their own rows — across multiple sessions of the same user — and
//!   never another user's rows or the superuser's.
//! - A foreign user cannot cancel another user's transaction or cursor
//!   without `ActivityView:Manage`, and the refusal leaves both targets
//!   usable; the same user's other session may cancel; a foreign user
//!   holding `Manage` may cancel too.
//!
//! The follower-side behaviour (ReadOnly node allows the read-only
//! listing per role) lives in the private handler test
//! `src/engine/db_handler/tests/policy/activity_follower_view_tests.rs`.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionStore};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
};
use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use shamir_test_diag::FlakeGuard;
use shamir_types::access::{ResourceMeta, ResourcePath};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::common::{
    cancel_cursor_admin_req, cancel_tx_req, create_cursor_req, encode, insert_req,
    list_activity_req, make_handler, make_session, on_big_stack, register_session, tx_begin_req,
    tx_insert_req, DB, REPO, TABLE,
};

const ALICE: &str = "act_alice";
const BOB: &str = "act_bob";

/// This test's own per-await bound (the persistent fixture is slower than the
/// in-memory engine, so it does not share `common::CALL_TIMEOUT`).
const TIMEOUT: Duration = Duration::from_secs(120);

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

/// Bound one await by [`TIMEOUT`]; no unbounded engine await in this test.
async fn bounded<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, fut)
        .await
        .unwrap_or_else(|_| panic!("{what} within {TIMEOUT:?}"))
}

async fn send(handler: &ShamirDbHandler, session: &Session, req: &DbRequest) -> DbResponse {
    let conn = ConnectionServices::without_push(0);
    let bytes = encode(req);
    let raw = bounded(
        "wire handler response",
        handler.handle(session, &bytes, &conn),
    )
    .await
    .expect("handle request");
    rmp_serde::from_slice(&raw).expect("decode DbResponse")
}

/// Grant `ActivityView` + `Manage` to exactly one ordinary user.
///
/// `ShamirDb::set_resource_meta` has no `ActivityView` arm (it rejects the
/// path with `NotFound`), so the only writable form of this singleton's meta
/// is the same setting key the authorization path reads:
/// `resource_meta(ActivityView)` loads `activity_view_meta` from the system
/// store and decodes it with `ResourceMeta::from_record`. Owner-only rwx
/// (`ResourceMeta::owned_enforced`) is the single-principal `Manage` rule.
async fn grant_activity_manage(shamir: &ShamirDb, session: &Session) {
    let meta = ResourceMeta::owned_enforced(Actor::User(principal64(session.user_id)));
    let mut map = new_map();
    map.insert(
        "key".to_string(),
        QueryValue::Str("activity_view_meta".to_string()),
    );
    let mut rec = QueryValue::Map(map);
    meta.inject_into(&mut rec);
    bounded(
        "save activity view meta",
        shamir
            .system_store()
            .save_setting("activity_view_meta", &rec),
    )
    .await
    .expect("grant ActivityView Manage to the foreign caller");
}

#[test]
fn permission_denial_activity() {
    // Keep the root alive until the runtime has shut down its background
    // tasks (mirrors `cancel_fixture::acceptance`).
    let root = tempfile::tempdir().expect("Fjall fixture directory");
    let path = root.path().to_owned();
    let guard = FlakeGuard::with_watchdog("permission_denial_activity", TIMEOUT);
    on_big_stack(move || async move {
        run(guard, &path).await;
    });
    drop(root);
}

async fn run(guard: FlakeGuard, root: &std::path::Path) {
    let shamir = Arc::new(
        bounded(
            "open system store",
            ShamirDb::init(SystemStoreConfig::Fjall(root.join("catalogue"))),
        )
        .await
        .expect("open system store"),
    );
    bounded("open seed catalogue", shamir.check_seed_catalogue())
        .await
        .expect("open seed catalogue");
    let store = Arc::new(SessionStore::new());
    let handler = make_handler(&shamir, Arc::clone(&store));

    let admin = register_session(&store, make_session("admin", 0xFE, true));
    let owner = Actor::User(principal64(admin.user_id));
    bounded("create db", shamir.create_db_as(DB, owner.clone()))
        .await
        .expect("create db");
    bounded(
        "create repo and table",
        shamir.add_repo_as(
            DB,
            RepoConfig::new(REPO, BoxRepoFactory::fjall_raw(root.join("data")))
                .add_table(TableConfig::new(TABLE)),
            owner,
        ),
    )
    .await
    .expect("create repo and table");
    for path in [
        ResourcePath::database(DB),
        ResourcePath::store(DB, REPO),
        ResourcePath::table(DB, REPO, TABLE),
    ] {
        bounded(
            "open data-path resource",
            shamir.set_resource_meta(&path, &ResourceMeta::open()),
        )
        .await
        .expect("allow ordinary fixture users to reach the data path");
    }

    guard.mark("alice-sessions");
    // Two sessions of the same ordinary user; the first holds a live tx
    // and an un-drained cursor so the Own scope has real rows to expose.
    // The cursor hands out COMMITTED rows, so seed four first: each of the
    // two cursors below needs a follow-up page, and the first is fetched
    // once as a positive post-state probe.
    let alice1 = register_session(&store, make_session(ALICE, 0xA1, false));
    let alice2 = register_session(&store, make_session(ALICE, 0xA2, false));
    for value in 0..4 {
        let resp = send(&handler, &admin, &insert_req(value)).await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "seed insert {value}: {resp:?}"
        );
    }
    let tx_handle = match send(&handler, &alice1, &tx_begin_req()).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("alice tx begin: {other:?}"),
    };
    assert!(matches!(
        send(&handler, &alice1, &tx_insert_req(tx_handle, 10)).await,
        DbResponse::TxBatch { .. }
    ));
    let cursor_id = match send(&handler, &alice1, &create_cursor_req()).await {
        DbResponse::CursorPage {
            cursor_id,
            has_more: true,
            ..
        } => cursor_id,
        other => panic!("seeded committed rows must leave the cursor a follow-up page: {other:?}"),
    };

    guard.mark("all-denied");
    // (a) Default denial: ordinary user, scope All, from BOTH alice
    // sessions — no ActivityView grant exists for either.
    for (label, session) in [("alice1", &alice1), ("alice2", &alice2)] {
        let resp = send(&handler, session, &list_activity_req(all_all())).await;
        assert!(
            matches!(resp, DbResponse::Error { ref code, .. } if code == "permission_denied"),
            "{label}: scope All without the ActivityView grant must be permission_denied"
        );
    }

    guard.mark("own-filtered");
    // (b) The superuser's `All` listing shows alice's rows — and alice's
    // `Own` listing shows ONLY alice's rows, never the admin's.
    let admin_view = match send(&handler, &admin, &list_activity_req(all_all())).await {
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

    let alice_view = match send(&handler, &alice2, &list_activity_req(own_all())).await {
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

    guard.mark("cancel-permissions");
    // (c) A foreign ordinary user WITHOUT `ActivityView:Manage` cannot
    // cancel alice's transaction or cursor, and both refusals leave the
    // targets untouched (positive post-state: the transaction is still
    // listed and still accepts a staged write; the cursor still fetches).
    let bob = register_session(&store, make_session(BOB, 0xB1, false));
    let denied_tx = send(&handler, &bob, &cancel_tx_req(tx_handle)).await;
    assert!(
        matches!(&denied_tx, DbResponse::Error { code, .. } if code == "permission_denied"),
        "bob cancelling alice's transaction: {denied_tx:?}"
    );
    let denied_cursor = send(&handler, &bob, &cancel_cursor_admin_req(cursor_id)).await;
    assert!(
        matches!(&denied_cursor, DbResponse::Error { code, .. } if code == "permission_denied"),
        "bob cancelling alice's cursor: {denied_cursor:?}"
    );

    let tx_listing = match send(
        &handler,
        &admin,
        &list_activity_req(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::Transactions,
            limit: None,
            continuation: None,
        }),
    )
    .await
    {
        DbResponse::ActivityPage { page } => page.rows,
        other => panic!("admin transactions: {other:?}"),
    };
    assert!(
        tx_listing.iter().any(|row| matches!(
            row,
            ActivityRow::Transaction { tx_handle: listed, .. } if *listed == tx_handle
        )),
        "a denied foreign cancel must leave alice's transaction open"
    );
    assert!(
        matches!(
            send(&handler, &alice1, &tx_insert_req(tx_handle, 11)).await,
            DbResponse::TxBatch { .. }
        ),
        "a denied foreign cancel must not disturb alice's transaction"
    );
    assert!(
        matches!(
            send(
                &handler,
                &alice1,
                &shamir_query_builder::cursor::fetch_next(cursor_id, None)
            )
            .await,
            DbResponse::CursorPage { .. }
        ),
        "a denied foreign cancel must leave alice's cursor fetchable"
    );

    guard.mark("cancel-own-user");
    // (d) Another session of the SAME user needs no Manage grant: it can
    // cancel the owner's transaction and cursor, and the owner's next use
    // of either fails typed.
    let cancelled_tx = send(&handler, &alice2, &cancel_tx_req(tx_handle)).await;
    assert!(
        matches!(&cancelled_tx, DbResponse::CancelOutcome { outcome } if *outcome == CancelOutcome::Cancelled),
        "alice-session-2 cancelling alice-session-1's transaction: {cancelled_tx:?}"
    );
    let gone = send(&handler, &alice1, &tx_insert_req(tx_handle, 12)).await;
    assert!(
        matches!(&gone, DbResponse::Error { code, .. } if code == "tx_not_found"),
        "the owner's next use of a cancelled handle: {gone:?}"
    );
    let cancelled_cursor = send(&handler, &alice2, &cancel_cursor_admin_req(cursor_id)).await;
    assert!(
        matches!(&cancelled_cursor, DbResponse::CancelOutcome { outcome } if *outcome == CancelOutcome::Cancelled),
        "alice-session-2 cancelling alice-session-1's cursor: {cancelled_cursor:?}"
    );
    let dead_cursor = send(
        &handler,
        &alice1,
        &shamir_query_builder::cursor::fetch_next(cursor_id, None),
    )
    .await;
    assert!(
        matches!(&dead_cursor, DbResponse::Error { code, .. } if code == "cursor_not_found"),
        "the owner's next fetch on a cancelled cursor: {dead_cursor:?}"
    );

    guard.mark("cancel-foreign-granted");
    // (e) Cross-user WITH `ActivityView:Manage`: same two ops, now allowed.
    // Each case consumes its target, so a fresh transaction and cursor are
    // opened by the (free) owning session.
    grant_activity_manage(&shamir, &bob).await;
    let granted_view = send(&handler, &bob, &list_activity_req(all_all())).await;
    assert!(
        matches!(granted_view, DbResponse::ActivityPage { .. }),
        "the Manage grant must also open scope All for bob: {granted_view:?}"
    );

    let tx_handle = match send(&handler, &alice1, &tx_begin_req()).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("second alice tx begin: {other:?}"),
    };
    assert!(matches!(
        send(&handler, &alice1, &tx_insert_req(tx_handle, 13)).await,
        DbResponse::TxBatch { .. }
    ));
    let granted_tx = send(&handler, &bob, &cancel_tx_req(tx_handle)).await;
    assert!(
        matches!(&granted_tx, DbResponse::CancelOutcome { outcome } if *outcome == CancelOutcome::Cancelled),
        "bob with ActivityView:Manage cancelling alice's transaction: {granted_tx:?}"
    );
    let gone = send(&handler, &alice1, &tx_insert_req(tx_handle, 14)).await;
    assert!(
        matches!(&gone, DbResponse::Error { code, .. } if code == "tx_not_found"),
        "the owner's use of a grant-cancelled handle: {gone:?}"
    );

    let cursor_id = match send(&handler, &alice1, &create_cursor_req()).await {
        DbResponse::CursorPage {
            cursor_id,
            has_more: true,
            ..
        } => cursor_id,
        other => panic!("seed rows must leave the second cursor a follow-up page: {other:?}"),
    };
    let granted_cursor = send(&handler, &bob, &cancel_cursor_admin_req(cursor_id)).await;
    assert!(
        matches!(&granted_cursor, DbResponse::CancelOutcome { outcome } if *outcome == CancelOutcome::Cancelled),
        "bob with ActivityView:Manage cancelling alice's cursor: {granted_cursor:?}"
    );
    let dead_cursor = send(
        &handler,
        &alice1,
        &shamir_query_builder::cursor::fetch_next(cursor_id, None),
    )
    .await;
    assert!(
        matches!(&dead_cursor, DbResponse::Error { code, .. } if code == "cursor_not_found"),
        "the owner's fetch on a grant-cancelled cursor: {dead_cursor:?}"
    );

    bounded("flush persistent fixture", shamir.flush_all_waiting())
        .await
        .expect("flush persistent fixture");
    guard.mark("done");
}
