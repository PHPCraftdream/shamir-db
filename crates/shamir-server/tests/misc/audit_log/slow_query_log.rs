//! Slow-query logging test.
//!
//! Sets `slow_query_threshold_ms = 1` (1 µs in practice — every batch is
//! "slow") and verifies that the handler emits a tracing::warn line with
//! the expected fields. Without an assertion like this the warning line
//! works on the honor system: we'd silently regress and only notice in
//! production when a slow query goes unflagged.

use std::sync::{Arc, Mutex};

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_server::db_handler::{DbRequest, ShamirDbHandler, SlowQueryConfig};
use tracing::Subscriber;
use tracing_subscriber::fmt::MakeWriter;

/// `MakeWriter` that appends every byte written by the tracing layer
/// into a shared `Vec<u8>` so the test can assert on the captured text.
#[derive(Clone)]
struct CaptureWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}
struct CaptureGuard {
    buf: Arc<Mutex<Vec<u8>>>,
}
impl std::io::Write for CaptureGuard {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureGuard;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureGuard {
            buf: self.buf.clone(),
        }
    }
}

fn make_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warn_line_emitted_when_query_exceeds_threshold() {
    // 1. Wire a tracing subscriber whose writer captures into a Vec.
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    // 2. Build a handler with `threshold_us = 1` — basically every batch
    // will exceed it (each takes at least a few microseconds).
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db("prod").await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("prod", cfg).await.expect("add repo");
    // G.4c: new objects default to enforced (0o700, System). Open the db +
    // repo ancestors so the non-superuser user session (Actor::User) can
    // traverse them. This test exercises slow-query logging, not access
    // control.
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::database("prod"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::store("prod", "main"),
            &open,
        )
        .await
        .unwrap();
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table("prod", "main", "items"),
            &open,
        )
        .await
        .unwrap();
    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_slow_query(SlowQueryConfig { threshold_us: 1 });

    // 3. Run a batch — should trigger the warn line.
    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("client-corr-99");
    b.query("rd", shamir_query_builder::Query::from("items"));
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch: b.build(),
    };
    let req_bytes = rmp_serde::to_vec_named(&req).expect("encode");
    let _ = handler
        .handle(
            &make_session(),
            &req_bytes,
            &ConnectionServices::without_push(0),
        )
        .await
        .expect("handle ok");

    // 4. Check the captured tracing output.
    let captured = String::from_utf8(buf.lock().unwrap().clone()).expect("utf8");
    assert!(
        captured.contains("slow query"),
        "expected 'slow query' WARN line; captured = {:?}",
        captured
    );
    // Field assertions — exact format depends on the formatter, but each
    // value should appear as `<field>=<value>` somewhere in the line.
    assert!(
        captured.contains("db=\"prod\""),
        "db field, got {:?}",
        captured
    );
    assert!(
        captured.contains("queries=1"),
        "queries field, got {:?}",
        captured
    );
    assert!(
        captured.contains("threshold_us=1"),
        "threshold_us field, got {:?}",
        captured
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_warn_when_threshold_is_zero() {
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let subscriber: Box<dyn Subscriber + Send + Sync> = Box::new(
        tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish(),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db("prod").await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("prod", cfg).await.expect("add repo");
    // threshold_us = 0 → DISABLED.
    let handler = ShamirDbHandler::new(Arc::new(shamir)).with_slow_query(SlowQueryConfig::DISABLED);

    let mut b = shamir_query_builder::batch::Batch::new();
    b.id("noop");
    b.query("rd", shamir_query_builder::Query::from("items"));
    let req = DbRequest::Execute {
        query_version: shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        db: "prod".into(),
        batch: b.build(),
    };
    let _ = handler
        .handle(
            &make_session(),
            &rmp_serde::to_vec_named(&req).unwrap(),
            &ConnectionServices::without_push(0),
        )
        .await
        .unwrap();

    let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        !captured.contains("slow query"),
        "no warn line expected when disabled; captured = {:?}",
        captured
    );
}
