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

use std::future::Future;
use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::activity::{ActivityKind, ActivityScope, ListActivityReq};

use crate::db_handler::config::NodeMode;
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
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
/// session source) over an opened in-memory database.
async fn follower_handler() -> (ShamirDbHandler, Arc<SessionStore>) {
    let db = ShamirDb::init_memory().await.expect("init memory database");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
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
    on_big_stack(|| async {
        let (handler, store) = follower_handler().await;
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
    on_big_stack(|| async {
        let (handler, store) = follower_handler().await;
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
