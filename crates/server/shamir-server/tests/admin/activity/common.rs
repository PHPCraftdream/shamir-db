//! Shared fixtures for the R4b-1 activity-view integration tests.
//!
//! Direct-handler harness (no TLS/SCRAM scaffolding): an in-memory
//! `ShamirDb` with an opened db/repo/table, a `ShamirDbHandler` with an
//! attached `SessionStore`, typed sessions registered as fixture sessions,
//! and request builders built exclusively through `shamir-query-builder`.
//! Requests go through the public `RequestHandler::handle` wire entry.

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
use shamir_query_types::admin::activity::ListActivityReq;

use shamir_server::db_handler::{DbRequest, DbResponse, ShamirDbHandler};
use shamir_server::version::CURRENT_QUERY_LANG_VERSION;

/// The standard fixture database coordinates.
pub(crate) const DB: &str = "app";
pub(crate) const REPO: &str = "main";
pub(crate) const TABLE: &str = "items";

/// Existing server fixture convention: deep engine futures need a roomy
/// stack; one runner per test, no runtime-inside-runtime.
pub(crate) fn on_big_stack<F, Fut>(body: F)
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

/// In-memory engine with `db/repo/table` created, owned by the fixture
/// admin principal, and opened (0o777) so ordinary users can read/write.
pub(crate) async fn open_engine() -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init memory database");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as(DB, owner.clone())
        .await
        .expect("create_db");
    shamir
        .add_repo_as(
            DB,
            RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE)),
            owner.clone(),
        )
        .await
        .expect("add repo");
    let open = shamir_types::access::ResourceMeta::open();
    for path in [
        shamir_types::access::ResourcePath::database(DB),
        shamir_types::access::ResourcePath::store(DB, REPO),
        shamir_types::access::ResourcePath::table(DB, REPO, TABLE),
    ] {
        shamir
            .set_resource_meta(&path, &open)
            .await
            .expect("open resource");
    }
    Arc::new(shamir)
}

/// Handler with the fixture session store attached (activity-view source
/// of session rows).
pub(crate) fn make_handler(shamir: &Arc<ShamirDb>, store: Arc<SessionStore>) -> ShamirDbHandler {
    ShamirDbHandler::new(Arc::clone(shamir)).with_session_store(store)
}

/// A typed fixture session for `username`; superuser flag per argument.
/// `tag` makes the 32-byte session id (bearer sid) unique per session.
/// The 16-byte user_id is derived STABLY from the username (same username
/// → same principal) so `Own`-scope filtering and per-user counting can
/// distinguish users; `admin` and ordinary users never collide.
pub(crate) fn make_session(username: &str, tag: u8, superuser: bool) -> Session {
    let mut user_id = [0u8; 16];
    for (i, b) in username.bytes().enumerate() {
        user_id[i % 16] = user_id[i % 16].wrapping_add(b);
    }
    let mut s = Session::new(
        user_id,
        username.into(),
        SessionPermissions::new(superuser, false, vec![]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    let mut sid = [0u8; 32];
    sid[0] = tag;
    s.session_id = sid;
    s
}

/// Register a session in the store (test-fixture admission; the store
/// stamps the minted activity handle). Returns the published generation
/// that handlers must observe.
pub(crate) fn register_session(store: &SessionStore, session: Session) -> Arc<Session> {
    store.insert(session.session_id, session)
}

pub(crate) fn encode(req: &DbRequest) -> Vec<u8> {
    rmp_serde::to_vec_named(req).expect("encode req")
}

/// Hard per-call bound: every fixture await is bounded by the test's own
/// timeout, never an unbounded engine await.
pub(crate) const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Drive one request through the public `handle` wire entry, bounded by
/// [`CALL_TIMEOUT`].
pub(crate) async fn call(
    handler: &ShamirDbHandler,
    session: &Session,
    req: &DbRequest,
) -> DbResponse {
    let conn = ConnectionServices::without_push(0);
    let raw = tokio::time::timeout(CALL_TIMEOUT, handler.handle(session, &encode(req), &conn))
        .await
        .expect("handle request within the per-call timeout")
        .expect("handle request");
    rmp_serde::from_slice(&raw).expect("decode DbResponse")
}

/// `count` committed rows with distinct `n` values (0..count) so result
/// cursors opened afterwards have pages to hand out.
pub(crate) async fn seed_committed_rows(handler: &ShamirDbHandler, admin: &Session, count: i64) {
    for i in 0..count {
        let resp = call(handler, admin, &insert_req(i)).await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "seed insert {i}: {resp:?}"
        );
    }
}

// --------------------------------------------------------------------------
// Request builders (shamir-query-builder only)
// --------------------------------------------------------------------------

fn execute(batch: shamir_query_builder::BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        batch,
    }
}

/// A committed single-row insert carrying `value` in field `n`.
pub(crate) fn insert_req(value: i64) -> DbRequest {
    let mut b = Batch::new();
    b.id("ins");
    b.insert(
        "i",
        shamir_query_builder::write::insert(TABLE)
            .rows([shamir_query_builder::doc! { "n" => value }]),
    );
    execute(b.build())
}

/// Open an interactive transaction on the fixture db/repo.
pub(crate) fn tx_begin_req() -> DbRequest {
    DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        repo: REPO.into(),
        isolation: None,
    }
}

/// Stage one insert of `value` inside an open transaction.
pub(crate) fn tx_insert_req(tx_handle: u64, value: i64) -> DbRequest {
    let mut b = Batch::new();
    b.id("txins");
    b.insert(
        "i",
        shamir_query_builder::write::insert(TABLE)
            .rows([shamir_query_builder::doc! { "n" => value }]),
    );
    DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        tx_handle,
        batch: b.build(),
    }
}

/// Open a one-row-page result cursor over the fixture table.
pub(crate) fn create_cursor_req() -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        query: shamir_query_builder::Query::from(TABLE).build(),
        page_size: 1,
    }
}

/// The typed `ListActivity` op as the sole entry of a standalone batch.
pub(crate) fn list_activity_req(request: ListActivityReq) -> DbRequest {
    let mut b = Batch::new();
    b.id("listact");
    b.op(
        "list_activity",
        shamir_query_builder::wire::admin::list_activity(request),
    );
    execute(b.build())
}
