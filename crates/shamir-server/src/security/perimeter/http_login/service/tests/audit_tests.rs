use super::super::LoginAudit;
use crate::http_login::metrics::LoginResult;
use serde_bytes::ByteBuf;
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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
async fn failures_count_only_fixed_classes() {
    let (appender, writer) = captured_writer();
    let (audit, task) = LoginAudit::start(CancellationToken::new(), writer);
    let minute_before = shamir_connect::common::time::UnixNanos::try_now()
        .expect("clock")
        .as_u64()
        / 60_000_000_000;
    audit.failure(LoginResult::AuthenticationFailed);
    audit.failure(LoginResult::AuthenticationFailed);
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
    assert_eq!(authentication, 2);
    assert_eq!(policy, 1);
    assert_eq!(invalid, 1);
}
