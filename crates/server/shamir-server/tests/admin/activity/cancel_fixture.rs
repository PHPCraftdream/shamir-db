//! Persistent direct-wire fixture for cooperative cancellation acceptance tests.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionStore};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::wire::admin::cancel_request;
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRequestState, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
    RequestExecId,
};
use shamir_server::db_handler::{DbRequest, DbResponse, QueryLimitsCap, ShamirDbHandler};
use shamir_test_diag::FlakeGuard;
use tokio_util::task::AbortOnDropHandle;

use super::common::{
    encode, execute, list_activity_req, make_session, on_big_stack, register_session, DB, REPO,
    TABLE,
};

const TIMEOUT: Duration = Duration::from_secs(120);
const STATUS_PACE: Duration = Duration::from_millis(20);

pub(super) fn acceptance<F, Fut>(name: &'static str, test: F)
where
    F: FnOnce(Arc<FlakeGuard>, Fixture) -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    // Keep the root alive until the runtime has shut down its background tasks.
    let root = tempfile::tempdir().expect("Fjall fixture directory");
    let path = root.path().to_owned();
    on_big_stack(move || async move {
        let guard = Arc::new(FlakeGuard::with_watchdog(name, TIMEOUT));
        let fixture = tokio::time::timeout(TIMEOUT, Fixture::open(&path))
            .await
            .expect("open persistent fixture within timeout");
        let db = Arc::clone(fixture.handler.db());
        tokio::time::timeout(TIMEOUT, test(Arc::clone(&guard), fixture))
            .await
            .expect("cancellation scenario within timeout");
        tokio::time::timeout(TIMEOUT, db.flush_all_waiting())
            .await
            .expect("flush persistent fixture within timeout")
            .expect("flush persistent fixture");
        guard.mark("done");
    });
    drop(root);
}

pub(super) struct Fixture {
    pub(super) handler: Arc<ShamirDbHandler>,
    store: Arc<SessionStore>,
    pub(super) admin: Arc<Session>,
}

impl Fixture {
    async fn open(root: &std::path::Path) -> Self {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("catalogue")))
            .await
            .expect("open system store");
        db.check_seed_catalogue()
            .await
            .expect("open seed catalogue");
        // These are pre-authenticated fixture sessions; SCRAM, rate limiting
        // at auth-init, and Argon2 admission are not on this handler path.
        let store = Arc::new(SessionStore::new());
        let admin = register_session(&store, make_session("admin", 0xFE, true));
        let owner = Actor::User(principal64(admin.user_id));
        db.create_db_as(DB, owner.clone()).await.expect("create db");
        db.add_repo_as(
            DB,
            RepoConfig::new(REPO, BoxRepoFactory::fjall_raw(root.join("data")))
                .add_table(TableConfig::new(TABLE)),
            owner,
        )
        .await
        .expect("create repo and table");
        for path in [
            shamir_types::access::ResourcePath::database(DB),
            shamir_types::access::ResourcePath::store(DB, REPO),
            shamir_types::access::ResourcePath::table(DB, REPO, TABLE),
        ] {
            db.set_resource_meta(&path, &shamir_types::access::ResourceMeta::open())
                .await
                .expect("allow ordinary fixture users to reach the data path");
        }
        let handler = ShamirDbHandler::new(Arc::new(db))
            .with_session_store(Arc::clone(&store))
            .with_query_limits(QueryLimitsCap {
                max_in_flight_requests_per_session: 1,
                ..QueryLimitsCap::UNLIMITED
            });
        Self {
            handler: Arc::new(handler),
            store,
            admin,
        }
    }

    pub(super) fn user(&self, name: &str, tag: u8) -> Arc<Session> {
        register_session(&self.store, make_session(name, tag, false))
    }

    pub(super) async fn send(&self, session: &Session, request: &DbRequest) -> DbResponse {
        tokio::time::timeout(TIMEOUT, send(&self.handler, session, request))
            .await
            .expect("control request within timeout")
    }

    /// A separately scheduled connection, so polling the control session
    /// cannot spuriously drive the target after it has registered its park.
    pub(super) fn start(
        &self,
        session: Arc<Session>,
        request: DbRequest,
    ) -> AbortOnDropHandle<DbResponse> {
        let handler = Arc::clone(&self.handler);
        AbortOnDropHandle::new(tokio::spawn(async move {
            send(&handler, &session, &request).await
        }))
    }

    pub(super) async fn requests(&self) -> Vec<ActivityRow> {
        let request = list_activity_req(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::Requests,
            limit: None,
            continuation: None,
        });
        match self.send(&self.admin, &request).await {
            DbResponse::ActivityPage { page } => page.rows,
            other => panic!("list activity: {other:?}"),
        }
    }

    /// Wait for a public state published by the target, or fail immediately
    /// on early completion. Polling the join handle does not poll the target.
    pub(super) async fn waiting(
        &self,
        pending: &mut AbortOnDropHandle<DbResponse>,
        target: &Session,
        expected: ActivityRequestState,
        ready: impl Fn() -> bool,
    ) -> RequestExecId {
        tokio::select! {
            biased;
            early = pending => panic!("request finished before {expected:?}: {early:?}"),
            id = async {
                loop {
                    let id = self.requests().await.into_iter().find_map(|row| match row {
                        ActivityRow::Request {
                            request_exec_id, session_handle: Some(handle), state, ..
                        } if handle == target.activity_handle() && state == expected => {
                            Some(request_exec_id)
                        }
                        _ => None,
                    });
                    if let Some(id) = id.filter(|_| ready()) {
                        break id;
                    }
                    tokio::time::sleep(STATUS_PACE).await;
                }
            } => id,
        }
    }

    pub(super) async fn cancel(&self, caller: &Session, id: RequestExecId) -> DbResponse {
        let mut batch = Batch::new();
        batch.op("cancel", cancel_request(id));
        self.send(caller, &execute(batch.build())).await
    }

    pub(super) async fn expect_cancel(
        &self,
        caller: &Session,
        id: RequestExecId,
        expected: CancelOutcome,
    ) {
        let response = self.cancel(caller, id).await;
        assert!(
            matches!(response, DbResponse::CancelOutcome { outcome } if outcome == expected),
            "cancel {id:?}: expected {expected:?}, got {response:?}"
        );
    }

    pub(super) async fn assert_retired(&self, target: &Session, id: RequestExecId) {
        assert!(!self.requests().await.iter().any(|row| matches!(row,
            ActivityRow::Request { request_exec_id, .. } if *request_exec_id == id)));
        assert!(
            target.try_acquire_request_slot(1).is_some(),
            "request slot leaked"
        );
        self.expect_cancel(&self.admin, id, CancelOutcome::AlreadyFinished)
            .await;
    }
}

async fn send(handler: &ShamirDbHandler, session: &Session, request: &DbRequest) -> DbResponse {
    let conn = ConnectionServices::without_push(0);
    let bytes = encode(request);
    let raw = Box::pin(handler.handle(session, &bytes, &conn))
        .await
        .expect("wire handler response");
    rmp_serde::from_slice(&raw).expect("decode typed response")
}
