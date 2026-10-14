use std::sync::Arc;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ports::RepoAdminObserver;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::lit;
use shamir_query_types::batch::BatchRequest;
use shamir_query_types::durability::{DurabilityOverride, WalCommitTier};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_types::types::value::QueryValue;

use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

#[derive(Default)]
struct MemoryAppender(parking_lot::Mutex<Vec<AuditEntry>>);

impl AuditAppender for MemoryAppender {
    fn append_entry(&self, entry: &AuditEntry) {
        self.0.lock().push(entry.clone());
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) {}
}

fn session(superuser: bool, user_id: [u8; 16]) -> Session {
    let mut session = Session::new(
        [3; 16],
        "alice".into(),
        SessionPermissions::new(superuser, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        UnixNanos::now().as_u64(),
    );
    session.session_id = [3; 32];
    session.user_id = user_id;
    session
}

async fn fixture() -> (ShamirDbHandler, Arc<MemoryAppender>, Session) {
    let db = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(principal64([3; 16]));
    db.create_db_as("app", owner.clone()).await;
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()),
        owner,
    )
    .await
    .unwrap();
    let chain = Arc::new(AuditChain::new([7; 32]));
    let appender = Arc::new(MemoryAppender::default());
    let writer = Arc::new(AuditChainWriter::new_with_shared(chain, appender.clone()));
    (
        ShamirDbHandler::new(Arc::new(db)).with_audit_writer(writer),
        appender,
        session(true, [3; 16]),
    )
}

fn alter_batch(alias: &str, repo: &str, return_result: bool) -> BatchRequest {
    let mut batch = Batch::new();
    let op = shamir_query_builder::ddl::alter_repo(repo).durability(Some(DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        ..Default::default()
    }));
    batch.alter_repo(alias, op);
    if !return_result {
        batch.return_only(Vec::<String>::new());
    }
    batch.build()
}

async fn send(
    handler: &ShamirDbHandler,
    session: &Session,
    batch: shamir_query_types::batch::BatchRequest,
) -> DbResponse {
    let request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch,
    };
    let bytes = rmp_serde::to_vec_named(&request).unwrap();
    let response = handler
        .handle(session, &bytes, &ConnectionServices::without_push(0))
        .await
        .unwrap();
    rmp_serde::from_slice(&response).unwrap()
}

#[tokio::test]
async fn observer_audits_effective_policy_even_when_result_is_suppressed() {
    let (handler, appender, session) = fixture().await;
    let response = send(&handler, &session, alter_batch("alter", "main", false)).await;
    assert!(matches!(response, DbResponse::Batch { response } if response.results.is_empty()));
    let entries = appender.0.lock();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.event, "alter_repo");
    assert_eq!(entry.user, "alice");
    assert_eq!(entry.transport, "tcp");
    assert_eq!(entry.ip_subnet, "");
    assert_eq!(&entry.session_id_prefix, &[3; 8]);
    assert_eq!(entry.result, "ok");
    let detail: QueryValue = rmp_serde::from_slice(&entry.details_canonical_msgpack).unwrap();
    let QueryValue::Map(fields) = detail else {
        panic!("audit details must be a map")
    };
    assert_eq!(fields.get("db"), Some(&QueryValue::Str("app".into())));
    assert_eq!(fields.get("repo"), Some(&QueryValue::Str("main".into())));
    let QueryValue::Map(policy) = fields.get("durability").unwrap() else {
        panic!("policy must be a map")
    };
    assert_eq!(
        policy.get("wal_commit"),
        Some(&QueryValue::Str("buffered".into()))
    );
    assert_eq!(
        policy.get("wal_retention_max_bytes"),
        Some(&QueryValue::Int(1_073_741_824))
    );
    assert_eq!(
        policy.get("on_wal_retention_exceeded"),
        Some(&QueryValue::Str("refuse_writes".into()))
    );
    assert_eq!(
        policy.get("replication_serve"),
        Some(&QueryValue::Str("durable".into()))
    );
    assert_eq!(entry.seq, 1);
    assert_eq!(entry.prev_hmac, [0; 32]);
    assert_ne!(entry.hmac, [0; 32]);
    assert!(AuditChain::verify_chain(&[7; 32], &entries).is_ok());
}

#[tokio::test]
async fn permission_and_invalid_alters_emit_no_audit_event() {
    let (handler, appender, _) = fixture().await;
    let denied = session(false, [4; 16]);
    let denied_response = send(&handler, &denied, alter_batch("denied", "main", true)).await;
    assert!(
        matches!(denied_response, DbResponse::Error { ref code, .. } if code == "access_denied")
    );
    assert!(appender.0.lock().is_empty());

    let permitted = session(true, [3; 16]);
    let mut invalid = Batch::new();
    invalid.alter_repo("invalid", shamir_query_builder::ddl::alter_repo("missing"));
    let invalid_response = send(&handler, &permitted, invalid.build()).await;
    assert!(matches!(invalid_response, DbResponse::Error { ref code, .. } if code == "query"));
    assert!(appender.0.lock().is_empty());
}

#[tokio::test]
async fn successful_alter_is_audited_before_a_later_batch_failure() {
    let (handler, appender, session) = fixture().await;
    let mut batch = Batch::new();
    let op = shamir_query_builder::ddl::alter_repo("main").durability(Some(DurabilityOverride {
        wal_commit: Some(WalCommitTier::Synced),
        ..Default::default()
    }));
    let alter = batch.alter_repo("alter", op);
    let later = batch.op(
        "later",
        shamir_query_builder::ddl::alter_repo("main").durability(Some(DurabilityOverride {
            wal_retention_max_bytes: Some(std::num::NonZeroU64::new(1024).unwrap()),
            ..Default::default()
        })),
    );
    batch.after(&later, &alter);
    let response = send(&handler, &session, batch.build()).await;
    assert!(matches!(response, DbResponse::Error { ref code, .. } if code == "invalid_request"));
    assert_eq!(appender.0.lock().len(), 1);
}

#[tokio::test]
async fn nested_batch_alter_is_audited() {
    let (handler, appender, session) = fixture().await;
    let mut inner = Batch::new();
    inner.alter_repo(
        "inner_alter",
        shamir_query_builder::ddl::alter_repo("main").durability(Some(DurabilityOverride {
            wal_commit: Some(WalCommitTier::Synced),
            ..Default::default()
        })),
    );
    let mut outer = Batch::new();
    outer.sub_batch_no_bind("nested", inner.build());
    let _ = send(&handler, &session, outer.build()).await;
    assert_eq!(appender.0.lock().len(), 1);
}

#[tokio::test]
async fn foreach_execution_audits_each_alter_iteration() {
    let (handler, appender, session) = fixture().await;
    let mut body = Batch::new();
    body.alter_repo(
        "inner_alter",
        shamir_query_builder::ddl::alter_repo("main").durability(Some(DurabilityOverride {
            wal_commit: Some(WalCommitTier::Synced),
            ..Default::default()
        })),
    );
    let mut outer = Batch::new();
    outer.for_each("loop", vec![lit(1), lit(2)], "item", body.build());
    let _ = send(&handler, &session, outer.build()).await;
    assert_eq!(appender.0.lock().len(), 2);
}

#[test]
fn audit_writer_builds_a_per_execution_observer() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handler, appender, session) = fixture().await;
        let observer = crate::db_handler::repo_audit::repo_audit_observer(
            handler.audit.as_ref().unwrap().clone(),
            &session,
        );
        let policy = shamir_db::engine::repo::DurabilityPolicy::default();
        observer.repository_durability_changed("app", "main", &policy);
        assert_eq!(appender.0.lock().len(), 1);
    });
}
