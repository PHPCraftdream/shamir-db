//! Follower (ReadOnly node) behaviour of the R4b-1 activity view
//! (design §5.2 / D13, D19): the read-only listing is allowed on a
//! follower according to the same permission split as a primary —
//! `scope: Own` is open to any authenticated user, `scope: All` still
//! requires the `ActivityView` grant (superuser passes; an ordinary user
//! is denied). No write gate applies: the listing is pure read-only state
//! introspection and must not be refused as a follower-write.
//!
//! Requests are built exclusively through `shamir-query-builder` and
//! driven through the public `RequestHandler::handle` wire entry.

use std::future::{poll_fn, Future};
use std::path::Path;
use std::sync::Arc;
use std::task::Poll;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRequestState, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
    RequestExecId,
};
use shamir_query_types::batch::BatchLimits;

use crate::byte_budget::ByteBudget;
use crate::db_handler::config::{NodeMode, QueryLimitsCap};
use crate::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// Existing server fixture convention: deep engine futures need a roomy
/// stack; one runner per test, no runtime-inside-runtime. The test body
/// runs on that roomy stack instead of the small `#[tokio::test]` stack.
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Hard per-call bound: every await is bounded, never an unbounded
/// engine await.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0xA1; 32];
    s
}

fn admin_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "admin".into(),
        SessionPermissions::new(true, false, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0xFE; 32];
    s
}

/// Follower handler with an attached session store (the activity view's
/// session source) over an opened persistent database.
async fn follower_handler(root: &Path) -> (ShamirDbHandler, Arc<SessionStore>) {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
        .await
        .expect("init persistent database");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
            .add_table(TableConfig::new("items")),
        owner.clone(),
    )
    .await
    .expect("add repo");
    let open = shamir_types::access::ResourceMeta::open();
    for path in [
        shamir_types::access::ResourcePath::database("app"),
        shamir_types::access::ResourcePath::store("app", "main"),
        shamir_types::access::ResourcePath::table("app", "main", "items"),
    ] {
        db.set_resource_meta(&path, &open)
            .await
            .expect("open resource");
    }
    let store = Arc::new(SessionStore::new());
    let handler = ShamirDbHandler::new(Arc::new(db))
        .with_node_mode(NodeMode::ReadOnly)
        .with_session_store(Arc::clone(&store));
    (handler, store)
}

fn list_req(scope: ActivityScope) -> DbRequest {
    let mut b = Batch::new();
    b.op(
        "list_activity",
        shamir_query_builder::wire::admin::list_activity(ListActivityReq {
            scope,
            kind: ActivityKind::All,
            limit: None,
            continuation: None,
        }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: b.build(),
    }
}

async fn call(handler: &ShamirDbHandler, session: &Session, req: &DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(req).expect("encode req");
    let conn = ConnectionServices::without_push(0);
    let raw = tokio::time::timeout(CALL_TIMEOUT, handler.handle(session, &bytes, &conn))
        .await
        .expect("handle request within the per-call timeout")
        .expect("handle request");
    rmp_serde::from_slice(&raw).expect("decode DbResponse")
}

/// `scope: Own` is open to any authenticated user on a follower: allowed,
/// returns a well-formed (possibly empty) page — never a follower-write
/// refusal or a permission denial.
#[test]
fn follower_allows_own_scope_listing() {
    let dir = tempfile::tempdir().expect("create persistent fixture directory");
    let root = dir.path().to_path_buf();
    on_big_stack(move || async move {
        let (handler, store) = tokio::time::timeout(CALL_TIMEOUT, follower_handler(&root))
            .await
            .expect("initialize follower fixture");
        let alice = store.insert(alice_session().session_id, alice_session());

        match call(&handler, &alice, &list_req(ActivityScope::Own)).await {
            DbResponse::ActivityPage { page } => {
                assert!(
                    page.continuation.is_none() || !page.rows.is_empty(),
                    "an exhausted page must carry no continuation"
                );
            }
            _ => panic!("follower must allow the read-only Own listing"),
        }
    });
}

/// The permission split is unchanged on a follower: the superuser's
/// `scope: All` listing is allowed; an ordinary user's is denied.
#[test]
fn follower_all_scope_follows_the_permission_split() {
    let dir = tempfile::tempdir().expect("create persistent fixture directory");
    let root = dir.path().to_path_buf();
    on_big_stack(move || async move {
        let (handler, store) = tokio::time::timeout(CALL_TIMEOUT, follower_handler(&root))
            .await
            .expect("initialize follower fixture");
        let admin = store.insert(admin_session().session_id, admin_session());
        let alice = store.insert(alice_session().session_id, alice_session());

        assert!(matches!(
            call(&handler, &admin, &list_req(ActivityScope::All)).await,
            DbResponse::ActivityPage { .. }
        ));

        assert!(matches!(
            call(&handler, &alice, &list_req(ActivityScope::All)).await,
            DbResponse::Error { ref code, .. } if code == "permission_denied"
        ));
    });
}

fn tiny_cancel_req(id: RequestExecId) -> DbRequest {
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: 1,
        ..BatchLimits::default()
    });
    batch.op(
        "cancel_request",
        shamir_query_builder::wire::admin::cancel_request(id),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: batch.build(),
    }
}

/// An unsigned cancellation by an ordinary user reaches the follower's
/// cancellation worker despite a saturated budget and session cap. The coarse
/// admin, follower-write, HMAC and encoded result cap gates must not block it.
#[test]
fn follower_allows_unsigned_cancel_request() {
    let dir = tempfile::tempdir().expect("create persistent fixture directory");
    let root = dir.path().to_path_buf();
    on_big_stack(move || async move {
        let (handler, store) = tokio::time::timeout(CALL_TIMEOUT, follower_handler(&root))
            .await
            .expect("initialize follower fixture");
        let alice = store.insert(alice_session().session_id, alice_session());
        let admin = store.insert(admin_session().session_id, admin_session());
        let budget = ByteBudget::new(Some(1));
        let reservation = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(1))
            .await
            .expect("reserve the whole budget before dispatch");
        assert_ne!(alice.activity_handle(), 0);
        let handler = handler
            .with_byte_budget(budget)
            .with_query_limits(QueryLimitsCap {
                max_in_flight_requests_per_session: 1,
                ..QueryLimitsCap::UNLIMITED
            });
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_result_size: 1,
            ..BatchLimits::default()
        });
        batch.query("read", Query::with_repo("main", "items"));
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: batch.build(),
        };
        // The live read owns Alice's only slot and parks on the held budget.
        let mut target = Box::pin(call(&handler, &alice, &req));
        poll_fn(|cx| {
            assert!(target.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(alice.try_acquire_request_slot(1).is_none());

        let DbResponse::ActivityPage { page } =
            call(&handler, &admin, &list_req(ActivityScope::All)).await
        else {
            panic!("expected ActivityPage while the budget is held");
        };
        let id = page
            .rows
            .iter()
            .find_map(|row| match row {
                ActivityRow::Request {
                    request_exec_id,
                    state: ActivityRequestState::WaitingBudget,
                    session_handle: Some(handle),
                    ..
                } if *handle == alice.activity_handle() => Some(*request_exec_id),
                _ => None,
            })
            .expect("follower read is parked on the byte budget");
        assert_ne!(id, RequestExecId(0));
        assert!(matches!(
            call(&handler, &alice, &tiny_cancel_req(RequestExecId(0))).await,
            DbResponse::CancelOutcome {
                outcome: CancelOutcome::AlreadyFinished,
            }
        ));
        assert!(matches!(
            call(&handler, &alice, &tiny_cancel_req(id)).await,
            DbResponse::CancelOutcome {
                outcome: CancelOutcome::Cancelled,
            }
        ));
        assert!(matches!(
            target.await,
            DbResponse::Error { code, .. } if code == "cancelled"
        ));
        assert!(alice.try_acquire_request_slot(1).is_some());
        drop(reservation);
    });
}
