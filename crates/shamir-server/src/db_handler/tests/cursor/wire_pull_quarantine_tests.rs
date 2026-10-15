use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_types::wire::repl::{ReplRequest, ReplResponse};
use tokio::sync::Notify;

use crate::db_handler::handler::ShamirDbHandler;
use crate::db_handler::repl_handler::{PullRetryHook, PULL_RETRY_HOOK};

#[tokio::test]
async fn pending_wire_long_poll_refuses_after_quarantine_flip() {
    let handler = build_handler().await;
    let db = handler.db();
    let hook = Arc::new(PullRetryHook {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let task_hook = hook.clone();
    let task_handler = handler.clone();
    let pending = tokio::spawn(async move {
        PULL_RETRY_HOOK
            .scope(task_hook, async move {
                task_handler
                    .handle_repl(
                        &alice_replicator_session(),
                        ReplRequest::Pull {
                            db: "app".into(),
                            repo: "main".into(),
                            from_version: u64::MAX,
                            limit: 10,
                            wait_ms: Some(30_000),
                            capabilities: None,
                        },
                    )
                    .await
            })
            .await
    });
    hook.entered.notified().await;

    let mut batch = Batch::new();
    batch.id(1);
    batch.op(
        "crp",
        shamir_query_builder::ddl::replication_profile("prof_retry").stream(
            shamir_query_builder::ddl::repl_scope("app")
                .repo("main")
                .build(),
            shamir_query_types::admin::ReplDirection::Pull,
            shamir_query_types::admin::ReplMode::ReadOnly,
        ),
    );
    batch.op(
        "cs",
        shamir_query_builder::ddl::subscription(
            "sub_retry",
            "tcp://leader:9000",
            "pub",
            "prof_retry",
        ),
    );
    db.execute("app", &batch.to_request_via_msgpack())
        .await
        .unwrap();
    db.mark_subscription_resync_required("sub_retry", 100, 42)
        .await
        .unwrap();
    hook.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        ReplResponse::Error { code, .. } if code == "downstream_resync_required"
    ));
}

fn alice_replicator_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, true, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        UnixNanos::now().as_u64(),
    )
}

async fn build_handler() -> Arc<ShamirDbHandler> {
    let db = ShamirDb::init_memory().await.expect("init database");
    let alice = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", alice.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        alice,
    )
    .await
    .expect("create owned repo");
    Arc::new(ShamirDbHandler::new(Arc::new(db)))
}
