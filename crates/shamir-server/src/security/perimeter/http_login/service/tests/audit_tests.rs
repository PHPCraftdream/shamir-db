use super::super::LoginAudit;
use crate::http_login::metrics::LoginResult;
use serde_bytes::ByteBuf;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct TraceCapture(Arc<Mutex<Vec<u8>>>);

struct TraceWriter(Arc<Mutex<Vec<u8>>>);

impl Write for TraceWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("trace capture lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for TraceCapture {
    type Writer = TraceWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TraceWriter(Arc::clone(&self.0))
    }
}

#[derive(Default)]
struct CaptureAppender(Mutex<Vec<AuditEntry>>);

impl AuditAppender for CaptureAppender {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.0.lock().expect("capture lock").push(entry.clone());
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn captured_writer() -> (Arc<CaptureAppender>, Arc<AuditChainWriter>) {
    let appender = Arc::new(CaptureAppender::default());
    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([8; 32]),
        appender.clone(),
    ));
    (appender, writer)
}

#[tokio::test]
async fn captures_issued_and_aggregate_events_without_identity() {
    let (appender, writer) = captured_writer();
    let (audit, task) = LoginAudit::start(CancellationToken::new(), writer);
    audit.issued([7; 16]);
    audit.failures(42, 3, 2, 1);
    drop(audit);
    task.await.expect("audit worker");

    let entries = appender.0.lock().expect("capture lock");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].event, "http_login_issued");
    assert_eq!(entries[0].user, "");
    assert_eq!(entries[0].ip_subnet, "");
    assert_eq!(
        rmp_serde::from_slice::<ByteBuf>(&entries[0].details_canonical_msgpack)
            .expect("token id")
            .as_ref(),
        &[7; 16]
    );
    assert_eq!(entries[1].event, "http_login_failures");
    assert_eq!(entries[1].user, "");
    assert_eq!(entries[1].ip_subnet, "");
    assert_eq!(
        rmp_serde::from_slice::<(u64, u32, u32, u32)>(&entries[1].details_canonical_msgpack)
            .expect("failure counts"),
        (42, 3, 2, 1)
    );
}

#[tokio::test]
async fn captures_compensation_event_with_token_id_only() {
    let (appender, writer) = captured_writer();
    let (audit, task) = LoginAudit::start(CancellationToken::new(), writer);
    audit.compensation_failed([9; 16]);
    drop(audit);
    task.await.expect("audit worker");

    let entries = appender.0.lock().expect("capture lock");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "http_login_compensation_failed");
    assert_eq!(entries[0].user, "");
    assert_eq!(entries[0].ip_subnet, "");
    assert_eq!(
        rmp_serde::from_slice::<ByteBuf>(&entries[0].details_canonical_msgpack)
            .expect("token id")
            .as_ref(),
        &[9; 16]
    );
}

#[tokio::test]
async fn process_budget_bounds_events_across_audit_consumers() {
    let (appender_a, writer_a) = captured_writer();
    let (appender_b, writer_b) = captured_writer();
    let budget = Arc::new(tokio::sync::Semaphore::new(1_024));
    let (audit_a, task_a) =
        LoginAudit::start_with_budget(CancellationToken::new(), writer_a, Arc::clone(&budget));
    let (audit_b, task_b) =
        LoginAudit::start_with_budget(CancellationToken::new(), writer_b, Arc::clone(&budget));
    for id in 0..1_024_u16 {
        if id % 2 == 0 {
            audit_a.issued([id as u8; 16]);
        } else {
            audit_b.issued([id as u8; 16]);
        }
    }
    audit_a.issued([0; 16]);
    assert_eq!(budget.available_permits(), 0);
    drop(audit_a);
    drop(audit_b);
    task_a.await.expect("first audit worker");
    task_b.await.expect("second audit worker");
    let entries_a = appender_a.0.lock().expect("capture lock");
    let entries_b = appender_b.0.lock().expect("capture lock");
    assert_eq!(entries_a.len(), 512);
    assert_eq!(entries_b.len(), 512);
    assert_eq!(entries_a.len() + entries_b.len(), 1_024);
    assert_eq!(budget.available_permits(), 1_024);
}

#[tokio::test]
async fn compensation_failure_has_critical_result() {
    let (appender, writer) = captured_writer();
    let (audit, task) = LoginAudit::start(CancellationToken::new(), writer);
    audit.compensation_failed([5; 16]);
    drop(audit);
    task.await.expect("audit worker");
    assert_eq!(
        appender.0.lock().expect("capture lock")[0].result,
        "critical"
    );
}

#[test]
fn origin_policy_diagnostic_is_count_only() {
    let capture = TraceCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            "{}",
            crate::http_login::HttpLoginRoutes::describe_origin_policy(0)
        );
        tracing::info!(
            "{}",
            crate::http_login::HttpLoginRoutes::describe_origin_policy(2)
        );
    });

    let output = String::from_utf8(capture.0.lock().expect("trace capture lock").clone())
        .expect("captured tracing output");
    assert!(output.contains("requests without Origin are allowed"));
    assert!(output.contains("requests with Origin are denied until an allowlist is configured"));
    assert!(output.contains("configured_origins=2"));
    assert!(!output.contains("https://origin-sentinel.invalid"));
}

#[tokio::test]
async fn failures_count_only_fixed_classes() {
    let (appender, writer) = captured_writer();
    let (audit, task) = LoginAudit::start(CancellationToken::new(), writer);
    let minute_before = shamir_connect::common::time::UnixNanos::try_now()
        .expect("clock")
        .as_u64()
        / 60_000_000_000;
    audit.failure(LoginResult::AuthenticationFailed);
    audit.failure(LoginResult::RateLimited);
    audit.failure(LoginResult::ServerBusy);
    audit.failure(LoginResult::PolicyDenied);
    audit.failure(LoginResult::InvalidRequest);
    audit.failure(LoginResult::Challenge);
    audit.failure(LoginResult::Issued);
    drop(audit);
    task.await.expect("audit worker");

    let entries = appender.0.lock().expect("capture lock");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event, "http_login_failures");
    assert_eq!(entries[0].user, "");
    assert_eq!(entries[0].ip_subnet, "");
    let details = &entries[0].details_canonical_msgpack;
    let minute_after = shamir_connect::common::time::UnixNanos::try_now()
        .expect("clock")
        .as_u64()
        / 60_000_000_000;
    let (minute, authentication, policy, invalid) =
        rmp_serde::from_slice::<(u64, u32, u32, u32)>(details).expect("failure counts");
    assert!((minute_before..=minute_after).contains(&minute));
    assert_eq!(authentication, 1);
    assert_eq!(policy, 1);
    assert_eq!(invalid, 1);
}
