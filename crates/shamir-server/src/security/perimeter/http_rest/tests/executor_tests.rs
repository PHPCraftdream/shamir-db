//! Behavioural regression tests for the concrete REST executor and scoped adapter.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use metrics_exporter_prometheus::PrometheusBuilder;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::token::{Endpoint, OwnerFloors, TokenChannels, TokenMode, Verified};
use shamir_connect::server::TokenSessionStore;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};
use shamir_query_types::wire::CURRENT_QUERY_LANG_VERSION;
use shamir_transport_http::{
    AuthHeader, Endpoint as HttpEndpoint, ExecStatus, GateDecision, PeerInfo, RestExecutor,
    TokenGate,
};
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::super::executor::{HandleDenial, RestHandler, ScopedHandler};
use super::super::gate::{RestGate, RestGateRails};
use crate::byte_budget::ByteBudget;
use crate::config::TokensConfig;
use crate::db_handler::ShamirDbHandler;
use crate::security::tokens::{NewToken, TokenService, TokenServiceOptions};

const NOW: u64 = 20_000_000_000;

struct Owners;
impl OwnerFloors for Owners {
    fn floor_if_present(&self, _: &[u8; 16]) -> Option<u64> {
        Some(0)
    }
}

struct Audit;
impl AuditAppender for Audit {
    fn append_entry(&self, _: &AuditEntry) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

async fn fixture(
    mode: TokenMode,
    databases: Option<Vec<String>>,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
) -> (
    TempDir,
    Arc<TokenService>,
    RestHandler,
    Verified,
    ByteBudget,
) {
    let temp = TempDir::new().expect("tempdir");
    let service = Arc::new(
        TokenService::open(TokenServiceOptions {
            data_dir: temp.path().to_path_buf(),
            token_key: Zeroizing::new([3; 32]),
            owners: Arc::new(Owners),
            sessions: Arc::new(TokenSessionStore::new()),
            audit: Arc::new(AuditChainWriter::new(
                AuditChain::new([2; 32]),
                Arc::new(Audit),
            )),
            config: TokensConfig::default(),
            read_only_node: false,
            clock: Arc::new(|| Some(NOW)),
        })
        .await
        .expect("token service"),
    );
    let created = service
        .create(NewToken {
            owner_user_id: [7; 16],
            owner_roles: vec!["reader".into()],
            label: "executor-test".into(),
            ttl_secs: Some(60),
            mode,
            channels: TokenChannels::Rest,
            databases,
        })
        .await
        .expect("create token");
    let verified = service
        .verify(created.token.as_bytes(), Endpoint::Rest, NOW)
        .expect("verify token");
    let database = Arc::new(ShamirDb::init_memory().await.expect("memory db"));
    database.create_db("inside").await.expect("create database");
    let budget = ByteBudget::new(Some(16 * 1024 * 1024));
    let handler = Arc::new(ShamirDbHandler::new(database).with_byte_budget(budget.clone()));
    let (gate, executor) = RestHandler::new(
        Arc::clone(&service),
        handler,
        clock,
        Arc::new(|_| "alice".to_owned()),
        RestGateRails {
            plaintext_remote: false,
            allow_plaintext_write: false,
        },
    );
    gate.shutdown().await;
    (temp, service, executor, verified, budget)
}

fn request(request: &shamir_query_types::wire::DbRequest) -> Bytes {
    Bytes::from(rmp_serde::to_vec_named(request).expect("encode request"))
}

fn response(outcome: &shamir_transport_http::ExecOutcome) -> shamir_query_types::wire::DbResponse {
    rmp_serde::from_slice(&outcome.body).expect("decode response")
}

#[tokio::test]
async fn executor_ping_returns_pong_and_malformed_bytes_are_400() {
    let (_temp, _service, executor, grant, _budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| Some(NOW))).await;
    let outcome = executor
        .execute(&grant, request(&shamir_query_types::wire::DbRequest::Ping))
        .await;
    assert!(matches!(outcome.status, ExecStatus::Ok));
    assert!(matches!(
        response(&outcome),
        shamir_query_types::wire::DbResponse::Pong
    ));
    let malformed = executor
        .execute(
            &grant,
            Bytes::from_static(b"shk1-secret-shaped-sensitive-payload"),
        )
        .await;
    assert!(matches!(malformed.status, ExecStatus::BadRequest));
    assert!(matches!(
        response(&malformed),
        shamir_query_types::wire::DbResponse::Error { code, message }
            if code == "invalid_request" && message == "invalid_request"
    ));
}

#[tokio::test]
async fn executor_visibility_and_ddl_status_engine_errors_remain_http_200() {
    let (_temp, _service, executor, grant, _budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| Some(NOW))).await;
    let wait = shamir_query_types::wire::DbRequest::WaitForVisibility {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "inside".into(),
        repo: "missing".into(),
        version: 0,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        executor.execute(&grant, request(&wait)),
    )
    .await
    .expect("missing-repository visibility wait must finish");
    assert!(matches!(outcome.status, ExecStatus::Ok));
    assert!(matches!(
        response(&outcome),
        shamir_query_types::wire::DbResponse::Error { .. }
    ));

    let status = shamir_query_types::wire::DbRequest::GetDdlOpStatus {
        db: "inside".into(),
        repo: "missing".into(),
        table: "missing".into(),
        op_id: "missing".into(),
    };
    let outcome = executor.execute(&grant, request(&status)).await;
    assert!(matches!(outcome.status, ExecStatus::Ok));
    assert!(matches!(
        response(&outcome),
        shamir_query_types::wire::DbResponse::Error { .. }
    ));
}

#[tokio::test]
async fn executor_read_only_token_denies_ddl_with_forbidden() {
    let (_temp, _service, executor, grant, _budget) = fixture(
        TokenMode::ReadOnly,
        Some(vec!["inside".into()]),
        Arc::new(|| Some(NOW)),
    )
    .await;
    let mut batch = Batch::new();
    batch.create_db(
        "create_blocked",
        shamir_query_builder::ddl::create_db("blocked"),
    );
    let ddl = shamir_query_types::wire::DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "inside".into(),
        batch: batch.build(),
    };
    let outcome = executor.execute(&grant, request(&ddl)).await;
    assert!(matches!(outcome.status, ExecStatus::Forbidden));
    assert!(matches!(
        response(&outcome),
        shamir_query_types::wire::DbResponse::Error { code, .. }
            if code == "token_read_only"
    ));
}

#[tokio::test]
async fn executor_enforces_read_only_database_and_rest_transaction_scope() {
    let (_temp, _service, executor, grant, _budget) = fixture(
        TokenMode::ReadOnly,
        Some(vec!["inside".into()]),
        Arc::new(|| Some(NOW)),
    )
    .await;
    let mut batch = Batch::new();
    batch.op("write", insert("items").row(doc()));
    let write = shamir_query_types::wire::DbRequest::Execute {
        query_version: 1,
        db: "inside".into(),
        batch: batch.build(),
    };
    let denied = executor.execute(&grant, request(&write)).await;
    assert!(matches!(denied.status, ExecStatus::Forbidden));
    assert!(matches!(
        response(&denied),
        shamir_query_types::wire::DbResponse::Error { code, .. }
            if code == "token_read_only"
    ));
    let outside = shamir_query_types::wire::DbRequest::WaitForVisibility {
        query_version: 1,
        db: "outside".into(),
        repo: "missing".into(),
        version: 1,
        visibility_wait: shamir_query_types::batch::VisibilityWait::Strict,
    };
    let denied = executor.execute(&grant, request(&outside)).await;
    assert!(matches!(denied.status, ExecStatus::Forbidden));
    assert!(matches!(
        response(&denied),
        shamir_query_types::wire::DbResponse::Error { code, .. }
            if code == "token_database_denied"
    ));
    let tx = shamir_query_types::wire::DbRequest::TxBegin {
        query_version: 1,
        db: "outside".into(),
        repo: "missing".into(),
        isolation: None,
    };
    let unsupported = executor.execute(&grant, request(&tx)).await;
    assert!(matches!(unsupported.status, ExecStatus::BadRequest));
    assert!(
        matches!(response(&unsupported), shamir_query_types::wire::DbResponse::Error { code, .. } if code == "not_supported_over_rest")
    );
}

#[tokio::test]
async fn executor_clock_failure_is_503_and_revocation_is_401() {
    let (_temp, service, executor, grant, _budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| Some(NOW))).await;
    service
        .revoke(
            shamir_connect::server::token::TokenId(grant.state.record().id),
            NOW + 1,
        )
        .await
        .expect("revoke");
    let revoked = executor
        .execute(&grant, request(&shamir_query_types::wire::DbRequest::Ping))
        .await;
    assert!(matches!(revoked.status, ExecStatus::Unauthorized));
    let (_temp, _service, unavailable, valid, _budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| None)).await;
    let failed = unavailable
        .execute(&valid, request(&shamir_query_types::wire::DbRequest::Ping))
        .await;
    assert!(matches!(failed.status, ExecStatus::Unavailable));
}

#[tokio::test]
async fn scoped_handler_cannot_call_native_handler_for_denied_scope() {
    let denied_calls = Arc::new(AtomicUsize::new(0));
    let denied_count = Arc::clone(&denied_calls);
    let scoped = ScopedHandler::with_handler(Arc::new(move |_, _, _| {
        denied_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { panic!("denied request reached native handler") })
    }));
    let token = TokenMode::ReadOnly;
    let (_temp, _service, _executor, verified, _budget) =
        fixture(token, None, Arc::new(|| Some(NOW))).await;
    let session = shamir_connect::server::session::Session::for_token(
        &verified,
        "alice".into(),
        NOW,
        [0; 32],
    );
    let conn = shamir_connect::server::conn_services::ConnectionServices::without_push(0);
    let denied = scoped
        .handle(
            &session,
            shamir_query_types::wire::DbRequest::TxBegin {
                query_version: 1,
                db: "db".into(),
                repo: "missing".into(),
                isolation: None,
            },
            Endpoint::Rest,
            &conn,
        )
        .await;
    assert_eq!(
        denied,
        Err(HandleDenial::BadRequest("not_supported_over_rest"))
    );
    assert_eq!(denied_calls.load(Ordering::SeqCst), 0);

    let native_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&native_calls);
    let allowed_handler = ScopedHandler::with_handler(Arc::new(move |_, _, _| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            rmp_serde::to_vec_named(&shamir_query_types::wire::DbResponse::Pong)
                .map_err(|error| error.to_string())
        })
    }));
    let allowed = allowed_handler
        .handle(
            &session,
            shamir_query_types::wire::DbRequest::Ping,
            Endpoint::Rest,
            &conn,
        )
        .await;
    assert!(matches!(
        allowed,
        Ok(bytes)
            if matches!(
                rmp_serde::from_slice::<shamir_query_types::wire::DbResponse>(&bytes),
                Ok(shamir_query_types::wire::DbResponse::Pong)
            )
    ));
    assert_eq!(native_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn executor_retains_byte_budget_hold_until_outcome_is_dropped() {
    let (_temp, _service, executor, grant, budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| Some(NOW))).await;
    let batch = Batch::new().build();
    let db_request = shamir_query_types::wire::DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "inside".into(),
        batch,
    };
    let encoded = request(&db_request);
    let outcome = executor.execute(&grant, encoded).await;
    assert!(matches!(outcome.status, ExecStatus::Ok));
    assert!(outcome.hold.is_some());
    assert_eq!(budget.used(), outcome.body.len());
    assert!(budget.used() > 0);
    drop(outcome);
    assert_eq!(budget.used(), 0);
}

fn auth_metric_counts(rendered: &str) -> (u64, u64, u64, u64) {
    for line in rendered
        .lines()
        .filter(|line| line.starts_with("shamir_token_auth_total{"))
    {
        assert!(
            line.contains("channel=\"rest\""),
            "unexpected auth label: {line}"
        );
        assert!(line.contains("result=\""), "missing auth result: {line}");
    }
    for line in rendered
        .lines()
        .filter(|line| line.starts_with("shamir_token_request_denied_total{"))
    {
        assert!(
            line.contains("endpoint=\"rest\""),
            "unexpected denial label: {line}"
        );
    }
    let count = |metric: &str, label: &str| {
        rendered
            .lines()
            .filter(|line| line.starts_with(metric) && line.contains(label))
            .filter_map(|line| line.rsplit_once(' '))
            .map(|(_, value)| value.parse::<u64>().expect("counter value"))
            .sum()
    };
    (
        count("shamir_token_auth_total{", "result=\"success\""),
        count("shamir_token_auth_total{", "result=\"forbidden\""),
        count("shamir_token_auth_total{", "result=\"unavailable\""),
        count("shamir_token_request_denied_total{", "endpoint=\"rest\""),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn successful_auth_and_scope_denial_use_separate_metrics() {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recorder_guard = metrics::set_default_local_recorder(&recorder);

    let (_temp, service, executor, _verified, _budget) =
        fixture(TokenMode::ReadWrite, None, Arc::new(|| Some(NOW))).await;
    let denied_token = service
        .create(NewToken {
            owner_user_id: [7; 16],
            owner_roles: vec!["reader".into()],
            label: "metrics-test".into(),
            ttl_secs: Some(60),
            mode: TokenMode::ReadOnly,
            channels: TokenChannels::Rest,
            databases: None,
        })
        .await
        .expect("create metrics token");
    let clock_available = Arc::new(AtomicBool::new(true));
    let gate_clock = Arc::clone(&clock_available);
    let gate = RestGate::new(
        Arc::clone(&service),
        Arc::new(move || gate_clock.load(Ordering::SeqCst).then_some(NOW)),
        RestGateRails {
            plaintext_remote: false,
            allow_plaintext_write: false,
        },
    );
    let header = format!("Bearer {}", denied_token.token.as_str());
    let peer = || PeerInfo {
        addr: "127.0.0.1:7340".parse().expect("peer address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let first = match gate
        .authenticate(
            AuthHeader::Single(header.as_bytes()),
            HttpEndpoint::Rest,
            peer(),
        )
        .await
    {
        GateDecision::Allow(grant) => grant,
        _ => panic!("gate rejected valid token"),
    };
    assert!(matches!(
        executor
            .execute(&first, request(&shamir_query_types::wire::DbRequest::Ping))
            .await
            .status,
        ExecStatus::Ok
    ));
    assert_eq!(auth_metric_counts(&handle.render()), (1, 0, 0, 0));
    let second = match gate
        .authenticate(
            AuthHeader::Single(header.as_bytes()),
            HttpEndpoint::Rest,
            peer(),
        )
        .await
    {
        GateDecision::Allow(grant) => grant,
        _ => panic!("gate rejected valid token"),
    };
    let mut batch = Batch::new();
    batch.create_db("blocked", shamir_query_builder::ddl::create_db("blocked"));
    let write = shamir_query_types::wire::DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "inside".into(),
        batch: batch.build(),
    };
    assert!(matches!(
        executor.execute(&second, request(&write)).await.status,
        ExecStatus::Forbidden
    ));
    assert_eq!(auth_metric_counts(&handle.render()), (2, 0, 0, 1));
    clock_available.store(false, Ordering::SeqCst);
    assert!(matches!(
        gate.revalidate(&first),
        GateDecision::Unavailable { .. }
    ));
    assert_eq!(auth_metric_counts(&handle.render()), (2, 0, 0, 1));
    gate.shutdown().await;
}
