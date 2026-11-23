//! `DbRequest::Status` handler tests (admin-shell P1 follow-up).
//!
//! Covers: the result-size cap applied to the encoded `Status` reply (typed
//! `result_too_large`, never a partial status), the per-session application
//! request slot (fixed `server_busy` while the only slot is held), the two
//! cheap gates (`permission_denied` for a non-superuser, `not_supported` when
//! the handler was built outside the server boot path), and the
//! first-pending pairing rule of `status_wal` (the age is present exactly
//! when the version is).
//!
//! Fixture: in-memory `ShamirDb` + `ShamirDbHandler::new`, `app` with two
//! repositories, boot metadata attached through the crate-private
//! `with_operator_boot`, requests dispatched through the real
//! `RequestHandler::handle`. Every await is bounded by a 120 s timeout.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_test_diag::try_install_global_tracing_ring;

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::durability_policy::DurabilityMetricsSnapshot;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_sync::format_stamp::StampOutcome;

use crate::db_handler::config::{QueryLimitsCap, SlowQueryConfig};
use crate::db_handler::handler::ShamirDbHandler;
use crate::engine::db_handler::operator_state::status_wal;

const ALICE_SID: [u8; 32] = [0xAA; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const BOOT_EPOCH: u64 = 7;

fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

fn superuser_alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

/// Handler over an in-memory `ShamirDb` with `app/{main,side}`, owned by
/// alice. Boot metadata is attached only when `with_boot` is set, so the
/// `not_supported` gate stays testable.
async fn build_handler(query_limits: QueryLimitsCap, with_boot: bool) -> ShamirDbHandler {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    for repo in ["main", "side"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner.clone()))
            .await
            .expect("add repo must complete within the 120s bound")
            .expect("add repo");
    }

    let handler = ShamirDbHandler::new(Arc::new(shamir)).with_query_limits(query_limits);
    if with_boot {
        handler.with_operator_boot(
            std::time::Instant::now(),
            BOOT_EPOCH,
            StampOutcome::Created,
            None,
        )
    } else {
        handler
    }
}

/// Send a wire request through the REAL `RequestHandler::handle`, bounded by
/// `CALL_TIMEOUT`.
async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = tokio::time::timeout(
        CALL_TIMEOUT,
        Box::pin(handler.handle(session, &bytes, &conn)),
    )
    .await
    .expect("handle must complete within the 120s bound")
    .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

fn expect_error(resp: DbResponse, code: &str, context: &str) {
    match resp {
        DbResponse::Error { code: got, .. } => assert_eq!(got, code, "{context}"),
        other => panic!("{context}: expected error code '{code}', got {other:?}"),
    }
}

/// Append sink copied from the shape-reset audit fixture: records every
/// `AuditChainWriter` append so the test can inspect the emitted entries.
#[derive(Default)]
struct CapturedAudit(Mutex<Vec<AuditEntry>>);

impl AuditAppender for CapturedAudit {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.0.lock().expect("audit mutex").push(entry.clone());
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn audit_writer() -> (Arc<CapturedAudit>, Arc<AuditChainWriter>) {
    let captured = Arc::new(CapturedAudit::default());
    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([0x42; 32]),
        captured.clone(),
    ));
    (captured, writer)
}

/// Ring records whose rendered message is a `slow query` WARN carrying
/// `needle`. `status` is server-wide, so its WARN carries `db=""`.
fn slow_query_records(needle: &str) -> Vec<shamir_test_diag::ring::RingRecord> {
    shamir_test_diag::ring::shared_ring()
        .snapshot()
        .into_iter()
        .filter(|record| {
            record.message.starts_with("slow query") && record.message.contains(needle)
        })
        .collect()
}

/// A status envelope is far bigger than 32 bytes, so a 32-byte cap must
/// refuse it whole with the typed `result_too_large`.
///
/// WITHOUT the fix `operator_status` never measures its reply, so the caller
/// gets a full `DbResponse::Status` even though the cap is active.
#[tokio::test]
async fn status_refuses_when_the_result_cap_is_exceeded() {
    let limits = QueryLimitsCap {
        enforce_result_size: true,
        max_result_size_bytes: 32,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(limits, true).await;
    let session = superuser_alice_session();

    let resp = send(&handler, &session, DbRequest::Status).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "result_too_large"),
        other => panic!("expected result_too_large, got {other:?}"),
    }
}

/// The handler must consult the session's application request slot exactly
/// like the other typed admin requests that hold a result: with the only
/// slot taken outside, the reply is the fixed `server_busy`; releasing the
/// slot lets the same request return a `Status`.
///
/// WITHOUT the fix the handler never takes the slot, so the first call
/// already returns a full status.
#[tokio::test]
async fn status_takes_the_application_request_slot() {
    let limits = QueryLimitsCap {
        max_in_flight_requests_per_session: 1,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(limits, true).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, superuser_alice_session());
    assert_ne!(session.activity_handle(), 0, "store-stamped session");

    let slot = session
        .try_acquire_request_slot(1)
        .expect("the cap-1 session admits exactly one external slot");
    let resp = send(&handler, &session, DbRequest::Status).await;
    expect_error(resp, "server_busy", "slot exhausted");
    drop(slot);

    let resp = send(&handler, &session, DbRequest::Status).await;
    assert!(
        matches!(resp, DbResponse::Status { .. }),
        "after the slot is released status succeeds: {resp:?}"
    );
}

/// Both cheap gates stay pinned: a non-superuser is `permission_denied`, and
/// a superuser on a handler built outside the boot path is `not_supported`.
#[tokio::test]
async fn status_requires_superuser_and_boot_context() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED, true).await;
    let non_superuser = alice_session();
    expect_error(
        send(&handler, &non_superuser, DbRequest::Status).await,
        "permission_denied",
        "non-superuser",
    );

    let no_boot = build_handler(QueryLimitsCap::UNLIMITED, false).await;
    let superuser = superuser_alice_session();
    expect_error(
        send(&no_boot, &superuser, DbRequest::Status).await,
        "not_supported",
        "handler without boot metadata",
    );
}

// ---------------------------------------------------------------------------
// First-pending pairing rule (pure; no live repository needed).
// ---------------------------------------------------------------------------

fn snapshot(first_pending: u64, first_pending_age: u64) -> DurabilityMetricsSnapshot {
    let mut gauges = [0u64; 15];
    gauges[9] = first_pending;
    gauges[10] = first_pending_age;
    DurabilityMetricsSnapshot {
        gauges,
        counters: [0u64; 9],
        journal: [0u64; 6],
        fsync: None,
    }
}

/// The age is present exactly when the version is. A zero age with a present
/// version is `Some(0)` — the same zero lower bound the metrics exporter
/// reports for that gauge — never `None`.
///
/// WITHOUT the fix the two fields were mapped independently, so a just-
/// observed (zero-age) pending span came back as `first_pending_age_ns: None`.
#[test]
fn status_wal_keeps_the_first_pending_pairing() {
    let wal = status_wal(&snapshot(0, 5_000));
    assert_eq!(wal.first_pending_version, None, "no pending version");
    assert_eq!(
        wal.first_pending_age_ns, None,
        "an age without a pending version must not surface"
    );

    let wal = status_wal(&snapshot(7, 0));
    assert_eq!(wal.first_pending_version, Some(7));
    assert_eq!(
        wal.first_pending_age_ns,
        Some(0),
        "a zero age is still a present age when the version is present"
    );

    let wal = status_wal(&snapshot(7, 42));
    assert_eq!(wal.first_pending_version, Some(7));
    assert_eq!(wal.first_pending_age_ns, Some(42));
}

// ---------------------------------------------------------------------------
// Durable audit + slow-query record for the typed `status` read.
// ---------------------------------------------------------------------------

/// One successful `status` emits exactly ONE durable audit record whose
/// payload carries no scoped names at all (the read is server-wide) and
/// nothing else — no status value, version, repository row or literal.
///
/// Catches: a missing enqueue (zero records), an enqueue on a refusal path
/// (permission/boot/slot), a second record for the same read, and any
/// payload field beyond the scope names.
#[tokio::test]
async fn status_emits_one_names_only_audit_record() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED, true).await;
    let session = superuser_alice_session();

    let (captured, writer) = audit_writer();
    let handler = handler.with_audit_writer(writer);

    let resp = send(&handler, &session, DbRequest::Status).await;
    assert!(
        matches!(resp, DbResponse::Status { .. }),
        "status must succeed: {resp:?}"
    );

    handler.cancel_refusal_audit().drain().await;
    let entries = captured.0.lock().expect("audit mutex");
    assert_eq!(
        entries.len(),
        1,
        "exactly one audit record per successful typed read"
    );
    let entry = &entries[0];
    assert_eq!(entry.event, "status");
    assert_eq!(entry.result, "ok");
    assert_eq!(entry.user, session.username);
    assert_eq!(&entry.session_id_prefix[..], &session.session_id[..8]);

    let details: serde_json::Value =
        rmp_serde::from_slice(&entry.details_canonical_msgpack).expect("decode details");
    let object = details.as_object().expect("details object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert!(
        keys.is_empty(),
        "status is server-wide: the payload carries no names, got {keys:?}"
    );
}

/// The typed `status` read logs the same `slow query` WARN the batch path
/// does: threshold 0 stays silent and a crossed threshold emits one record
/// for the server-wide `db=""` with `queries=1`.
///
/// Catches: no WARN at all (the parent behaviour), a disabled threshold that
/// still logs, and a WARN missing the mirror field names.
#[tokio::test]
async fn status_logs_slow_query_past_the_threshold_and_not_when_disabled() {
    try_install_global_tracing_ring();
    let session = superuser_alice_session();

    let disabled = build_handler(QueryLimitsCap::UNLIMITED, true)
        .await
        .with_slow_query(SlowQueryConfig::from_ms(0));
    let before = slow_query_records("db=\"\"").len();
    assert!(matches!(
        send(&disabled, &session, DbRequest::Status).await,
        DbResponse::Status { .. }
    ));
    assert_eq!(
        slow_query_records("db=\"\"").len(),
        before,
        "threshold 0 must disable the WARN"
    );

    // A 1 µs threshold is crossed by the real read without any sleep; the
    // millisecond granularity of `from_ms` could not be made deterministic.
    let enabled = build_handler(QueryLimitsCap::UNLIMITED, true)
        .await
        .with_slow_query(SlowQueryConfig { threshold_us: 1 });
    assert!(matches!(
        send(&enabled, &session, DbRequest::Status).await,
        DbResponse::Status { .. }
    ));

    let records = slow_query_records("db=\"\"");
    assert_eq!(
        records.len(),
        before + 1,
        "the crossed threshold must emit exactly one WARN"
    );
    let record = records.last().expect("slow query record");
    assert_eq!(record.level, "WARN");
    assert!(record.message.contains("slow query"), "{}", record.message);
    assert!(
        record.message.contains("threshold_us=1"),
        "{}",
        record.message
    );
    assert!(record.message.contains("queries=1"), "{}", record.message);
}
