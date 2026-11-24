//! Read-outcome audit coverage for the typed `DbRequest::GetRecord` point
//! read: one names-only audit record per successful read, and the `slow
//! query` WARN mirroring the batch path.
//!
//! Moved out of `cursor/get_record_tests.rs` (layout cap) together with the
//! fixture helpers those two tests need. The fixture mirrors that file's
//! in-memory `app/{main,side}.items` handler; every await stays bounded by
//! the same 120 s timeout.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::instrument::WithSubscriber;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor, ResourceMeta, ResourcePath};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::insert;
use shamir_query_types::read::QueryRecord;
use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_types::types::record_id::RecordId;

use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap, SlowQueryConfig};
use crate::db_handler::handler::ShamirDbHandler;

const ALICE_SID: [u8; 32] = [0xAA; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

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

fn bob_session() -> Session {
    let mut s = Session::new(
        [0xAC; 16],
        "bob".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = [0xBB; 32];
    s
}

/// Handler over an in-memory `ShamirDb` with `app/{main,side}.items`,
/// owned by alice (principal `[0xAB; 16]` — same identity the alice
/// session carries, so alice is the owner). `main` is seeded with one
/// row `{ id: "k000", v: 0 }`; `side` stays empty. The database and
/// stores are open while the target table remains at its enforced 0o700 default.
async fn build_handler(query_limits: QueryLimitsCap) -> ShamirDbHandler {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    let open = ResourceMeta::open();
    tokio::time::timeout(
        CALL_TIMEOUT,
        shamir.set_resource_meta(&ResourcePath::database("app"), &open),
    )
    .await
    .expect("open database must complete within the 120s bound")
    .expect("open database");
    for repo in ["main", "side"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner.clone()))
            .await
            .expect("add repo must complete within the 120s bound")
            .expect("add repo");
    }
    tokio::time::timeout(
        CALL_TIMEOUT,
        shamir.set_resource_meta(&ResourcePath::store("app", "main"), &open),
    )
    .await
    .expect("open store must complete within the 120s bound")
    .expect("open store");

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed_k000",
        insert("items").row(doc! { "id" => "k000", "v" => 0_i64 }),
    );
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("seed row");

    ShamirDbHandler::new(Arc::new(shamir))
        .with_cursor_limits(CursorLimitsCap::UNLIMITED)
        .with_query_limits(query_limits)
}

/// Send a wire request through the REAL `RequestHandler::handle`,
/// bounded by `CALL_TIMEOUT`.
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

/// Insert one row through the ENGINE's real `execute_as` path and return
/// its STORAGE `RecordId` — the typed `QueryRecord::Inserted` /
/// `InsertedRecord::id` the write result carries, never any payload
/// field. The insert deliberately does NOT go through the handler's
/// wire path: the result-size fixtures configure an 8-byte
/// `max_result_size_bytes` cap on the handler, which would reject the
/// insert's OWN batch response with `result_too_large` before any
/// point read could ever be sent. Seeding happens below the fixture's
/// budget so only the measured request is under the cap.
async fn insert_row_and_get_storage_id(handler: &ShamirDbHandler, payload_id: &str) -> RecordId {
    let mut b = Batch::new();
    b.insert(
        "row",
        insert("items").row(doc! { "id" => payload_id, "v" => 1_i64 }),
    );
    let response = tokio::time::timeout(
        CALL_TIMEOUT,
        handler.db().execute_as(Actor::System, "app", &b.build()),
    )
    .await
    .expect("engine insert must complete within the 120s bound")
    .expect("engine insert must succeed");
    let result = response.results.get("row").expect("insert alias returned");
    match result.records.first() {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    }
}

fn get_record_req(key: RecordId) -> DbRequest {
    get_record("app", "main", "items", key)
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

/// Test-owned capture for slow-query WARN events, isolated from other tests.
type SlowQueryRecords = Arc<Mutex<Vec<(String, String)>>>;

#[derive(Clone)]
struct SlowQueryCapture(SlowQueryRecords);

#[derive(Default)]
struct EventFields(Vec<String>);

impl Visit for EventFields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push(format!("{}={value:?}", field.name()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.push(format!("{}={value}", field.name()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.push(format!("{}={value}", field.name()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.push(format!("{}={value}", field.name()));
    }
}

impl<S: Subscriber> Layer<S> for SlowQueryCapture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut fields = EventFields::default();
        event.record(&mut fields);
        let message = fields.0.join(" ");
        if message.contains("slow query") {
            self.0
                .lock()
                .expect("slow-query capture mutex")
                .push((event.metadata().level().to_string(), message));
        }
    }
}

fn slow_query_dispatch() -> (tracing::Dispatch, SlowQueryRecords) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(SlowQueryCapture(records.clone()));
    (tracing::Dispatch::new(subscriber), records)
}

/// Carry the test dispatch into the task that polls the request future.
async fn send_captured(
    handler: &ShamirDbHandler,
    session: &Session,
    req: DbRequest,
    dispatch: &tracing::Dispatch,
) -> DbResponse {
    send(handler, session, req)
        .with_subscriber(dispatch.clone())
        .await
}

// ---------------------------------------------------------------------------
// Durable audit + slow-query record for the typed point read.
// ---------------------------------------------------------------------------

/// One successful `get_record` emits exactly ONE durable audit record whose
/// payload carries the scope NAMES only — never the record value, the typed
/// key or any caller literal.
///
/// Catches: a missing enqueue (zero records), an enqueue on refusal paths,
/// a second record for the same read, and any payload field beyond
/// db/repo/table (especially the returned value or the seeded payload
/// literal).
#[tokio::test]
async fn get_record_emits_one_names_only_audit_record() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();
    let storage_id = insert_row_and_get_storage_id(&handler, "k001").await;

    let (captured, writer) = audit_writer();
    let handler = handler.with_audit_writer(writer);

    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    assert!(
        matches!(resp, DbResponse::Record { .. }),
        "the point read must succeed: {resp:?}"
    );

    assert!(
        handler.cancel_refusal_audit().drain().await,
        "audit queue did not drain"
    );
    let entries = captured.0.lock().expect("audit mutex");
    assert_eq!(
        entries.len(),
        1,
        "exactly one audit record per successful typed read"
    );
    let entry = &entries[0];
    assert_eq!(entry.event, "get_record");
    assert_eq!(entry.result, "ok");
    assert_eq!(entry.user, session.username);
    assert_eq!(&entry.session_id_prefix[..], &session.session_id[..8]);

    let details: serde_json::Value =
        rmp_serde::from_slice(&entry.details_canonical_msgpack).expect("decode details");
    let object = details.as_object().expect("details object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["db", "repo", "table"],
        "the record carries the scope names only"
    );
    assert_eq!(object["db"].as_str(), Some("app"));
    assert_eq!(object["repo"].as_str(), Some("main"));
    assert_eq!(object["table"].as_str(), Some("items"));

    let encoded = &entry.details_canonical_msgpack;
    assert!(
        !encoded
            .windows(b"k001".len())
            .any(|window| window == b"k001"),
        "the seeded payload literal must never enter the audit chain"
    );
}

/// A refused point read leaves the durable audit chain untouched: a non-owner
/// is stopped by the table gate BEFORE the success observer, so the refusal
/// emits neither the `ok` audit record nor the `slow query` WARN observation.
#[tokio::test]
async fn get_record_refusal_emits_no_audit_record() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let storage_id = insert_row_and_get_storage_id(&handler, "k001").await;

    let (captured, writer) = audit_writer();
    let handler = handler.with_audit_writer(writer);

    let session = bob_session();
    let mut responses = Vec::new();
    for table in ["absent", "items"] {
        if table == "items" {
            let closed = ResourceMeta {
                owner: Actor::User(principal64([0xAB; 16])),
                group: None,
                mode: 0o700,
            };
            tokio::time::timeout(
                CALL_TIMEOUT,
                handler
                    .db()
                    .set_resource_meta(&ResourcePath::table("app", "main", "items"), &closed),
            )
            .await
            .expect("close table must complete within the 120s bound")
            .expect("close table");
        }
        let request = if table == "absent" {
            get_record("app", "main", "absent", storage_id)
        } else {
            get_record_req(storage_id)
        };
        let resp = send(&handler, &session, request).await;
        match resp {
            DbResponse::Error { code, message } => responses.push((code, message)),
            other => panic!("expected unknown_table for unavailable table, got {other:?}"),
        }
        assert!(
            captured.0.lock().expect("audit mutex").is_empty(),
            "each refused point read must leave the audit chain untouched"
        );
    }
    assert_eq!(responses[0].0, "unknown_table");
    assert_eq!(responses[0].1, "table is unavailable");
    assert_eq!(
        responses[0], responses[1],
        "refusals must be indistinguishable"
    );

    assert!(
        handler.cancel_refusal_audit().drain().await,
        "audit queue did not drain"
    );
    assert!(
        captured.0.lock().expect("audit mutex").is_empty(),
        "refused point reads must not emit an audit record"
    );
}

/// The typed point read logs the same `slow query` WARN the batch path does:
/// threshold 0 stays silent and a crossed threshold emits one record for
/// `db="app"` with `queries=1`.
///
/// Catches: no WARN at all (the parent behaviour), a disabled threshold that
/// still logs, and a WARN missing the mirror field names.
#[tokio::test]
async fn get_record_logs_slow_query_past_the_threshold_and_not_when_disabled() {
    let (dispatch, captured) = slow_query_dispatch();
    let session = alice_session();

    let disabled = build_handler(QueryLimitsCap::UNLIMITED)
        .await
        .with_slow_query(SlowQueryConfig::from_ms(0));
    let id = insert_row_and_get_storage_id(&disabled, "k101").await;
    assert!(matches!(
        send_captured(&disabled, &session, get_record_req(id), &dispatch).await,
        DbResponse::Record { .. }
    ));
    assert!(
        captured
            .lock()
            .expect("slow-query capture mutex")
            .is_empty(),
        "threshold 0 must disable the WARN"
    );

    // The elapsed value is pinned, so the WARN decision never depends on the
    // real read.
    let enabled = build_handler(QueryLimitsCap::UNLIMITED)
        .await
        .with_slow_query(SlowQueryConfig {
            threshold_us: 1_000,
        })
        .with_slow_query_elapsed(1_001);
    let id = insert_row_and_get_storage_id(&enabled, "k102").await;
    assert!(matches!(
        send_captured(&enabled, &session, get_record_req(id), &dispatch).await,
        DbResponse::Record { .. }
    ));

    let records = captured.lock().expect("slow-query capture mutex");
    assert_eq!(records.len(), 1, "exactly one slow-query WARN");
    let (level, message) = &records[0];
    assert_eq!(level, "WARN");
    assert!(message.contains("slow query"), "{message}");
    assert!(message.contains("threshold_us=1000"), "{message}");
    assert!(message.contains("db=\"app\""), "{message}");
    assert!(message.contains("queries=1"), "{message}");
}
