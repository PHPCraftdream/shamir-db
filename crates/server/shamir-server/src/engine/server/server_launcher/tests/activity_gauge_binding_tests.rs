//! R4b-1 gauge-binding test (release gate: "the operator can find a stuck
//! query"): the four `shamir_active_*` gauges must be bound to THIS boot's
//! request handler through `ObservabilityState::set_activity_source`,
//! installed by the production `ServerLauncher::launch` path.
//!
//! The test drives the REAL production boot end to end — `ServerLauncher`
//! over a real data dir, real observability HTTP server on an OS-picked
//! port — and then, through the `#[cfg(test)]` `ActivityGaugeTestSeam`
//! captured in `ServerHandle` (the same handler `Arc`, `ByteBudget` and
//! server-wide `SessionStore` the launcher handed its listeners), seeds:
//!
//! * 3 distinct stamped sessions in the actual session store,
//! * 1 real interactive transaction in the actual tx registry,
//! * 2 real un-drained cursors through the actual `CreateCursor` wire path,
//! * 4 ordinary write batches parked deterministically at the shared
//!   byte budget (the budget is held at full capacity first, so each
//!   batch parks AFTER in-flight registration — one explicit
//!   `Poll::Pending` poll per future proves the hold without any race
//!   polling; the futures stay alive, never polled again).
//!
//! The expected gauge counts 4/3/1/2 are then read from a REAL loopback
//! HTTP `GET /metrics` scrape of the address the observability server
//! actually bound (`ObservabilityHandle::bound_addr`, port 0 at config).
//! The launcher's `set_activity_source` call is the subject: if it were
//! omitted the recorder falls back to its default (unbound) source, which
//! still renders the family — but at all zeros, so the 4/3/1/2 asserts
//! below fail and expose the missing binding.
//!
//! After the scrape the held budget permit and the parked futures are
//! dropped: every in-flight RAII entry unwinds, so a second scrape shows
//! the non-live baseline (requests back to 0; sessions, the still-open
//! transaction and the still-open cursors remain — they were seeded
//! directly and are released only by shutdown).

use std::future::{poll_fn, Future};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_query_types::batch::BatchLimits;
use shamir_query_types::wire::{CursorId, DbRequest};
use shamir_test_diag::{try_install_global_tracing_ring, FlakeGuard};

use crate::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ObservabilityConfig, TlsConfig,
};
use crate::db_handler::{DbResponse, NodeMode, ShamirDbHandler};
use crate::server::{BootstrapMode, ServerLauncher};
use crate::tx_registry::InteractiveTx;
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// Hard per-call bound: every await is bounded, never an unbounded
/// engine await.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Distinctive (non-credential) literal inside the parked writes' rows.
const CANARY: &str = "GAUGE-BINDING-CANARY-4b1";

/// Existing launcher/boot test convention: deep boot and engine futures
/// need a roomy stack; one runner per test, no runtime-inside-runtime.
fn on_big_stack<F, Fut>(body: F)
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

/// Full production-launcher config over a real temp data dir: one plain
/// loopback HTTP data listener (no TLS material needed), observability on
/// port 0 (the ACTUAL bound port is read through the test seam, never
/// picked or retried by this test), and a finite in-flight response-byte
/// budget the test can hold at full capacity.
fn boot_config(data_dir: std::path::PathBuf) -> Config {
    Config {
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Http,
            addr: "127.0.0.1:0".into(),
            profile: None,
            http: crate::boot::config::HttpListenerSettings {
                tls: Some(false),
                ..Default::default()
            },
            path: None,
            kdf_override: None,
            browser_origin_allowlist: Vec::new(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: crate::config::SecurityConfig {
            query_limits: crate::config::QueryLimitsConfig {
                max_result_size_bytes: 4096,
                max_inflight_response_bytes: Some(16_384),
                ..Default::default()
            },
            ..Default::default()
        },
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: "127.0.0.1:0".into(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// A distinct stamped session: fixed distinct sid bytes, per-user id.
/// The session timestamp is NOW (not a fixed past value) so the real
/// boot's GC sweep cannot expire the seeded sessions mid-test.
fn stamped_session(tag: u8, name: &str, user: [u8; 16]) -> Session {
    let mut s = Session::new(
        user,
        name.into(),
        SessionPermissions::new(true, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    let mut sid = [0u8; 32];
    sid[0] = tag;
    sid[31] = tag.wrapping_mul(7).wrapping_add(1);
    s.session_id = sid;
    s
}

/// Raw bounded HTTP GET over loopback (the observability server speaks
/// plain HTTP; `Connection: close` bounds the read). The response head is
/// checked for a real `HTTP/1.1 200` status line; only the body is
/// returned.
async fn http_get(addr: SocketAddr, path: &str) -> String {
    let fut = async {
        let mut s = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect /metrics");
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.expect("write request");
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.expect("read response");
        let hdr_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(buf.len());
        let head = String::from_utf8_lossy(&buf[..hdr_end]).into_owned();
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "/metrics must answer HTTP 200, got head: {head:?}"
        );
        buf[hdr_end + 4..].to_vec()
    };
    tokio::time::timeout(CALL_TIMEOUT, fut)
        .await
        .expect("bounded metrics scrape")
        .iter()
        .map(|b| *b as char)
        .collect()
}

/// Parse `shamir_active_<name> <value>` lines out of the Prometheus text.
/// Even with the activity source unbound the recorder's zero-touch
/// default still renders the family (at all zeros), so absence of a
/// series is a failure of the export path itself.
fn gauge_value(metrics: &str, name: &str) -> u64 {
    for line in metrics.lines() {
        if let Some(rest) = line.strip_prefix(name).and_then(|r| r.strip_prefix(' ')) {
            return rest
                .trim()
                .parse()
                .unwrap_or_else(|e| panic!("gauge {name}: bad value {rest:?}: {e}"));
        }
    }
    panic!("gauge {name} missing from /metrics — the activity family itself failed to render");
}

#[test]
fn activity_gauges_scrape_live_counts_from_the_production_boot_handler() {
    on_big_stack(|| async {
        // The whole body is bounded: several setup awaits below (launch,
        // DDL, tx begin) are individually unbounded, so the entire test is
        // wrapped in one hard timeout instead.
        tokio::time::timeout(Duration::from_secs(180), run())
            .await
            .expect("entire gauge-binding test bounded");
    });
}

/// The whole test body, factored out of the `#[test]` wrapper: several
/// setup awaits (launch, DDL, tx begin) are individually unbounded, so the
/// test wraps this one call in a single hard timeout.
async fn run() {
    let guard = FlakeGuard::with_watchdog(
        "activity_gauges_scrape_live_counts_from_the_production_boot_handler",
        Duration::from_secs(150),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("tempdir");
    let launcher = ServerLauncher {
        config: boot_config(temp.path().to_path_buf()),
        bootstrap: BootstrapMode::Skip,
        node_mode: NodeMode::ReadWrite,
    };
    let handle = launcher.launch().await.expect("production boot");
    guard.mark("server launched");

    // The seam is cfg(test)-only and populated by launch() whenever an
    // observability server was configured (this config always runs one);
    // reading the REAL handler/budget/store/address out of it is what
    // makes this a binding test, not a parallel fixture.
    let seam = handle
        .activity_test_seam
        .as_ref()
        .expect("test seam captured");
    let handler: Arc<ShamirDbHandler> = Arc::clone(&seam.handler);
    let store = Arc::clone(&seam.session_store);
    let obs_addr = seam.observability_bound_addr;
    assert_eq!(obs_addr.ip(), std::net::IpAddr::from([127, 0, 0, 1]));
    assert_ne!(
        obs_addr.port(),
        0,
        "config said port 0; the server must have bound a real one"
    );
    guard.note("observability bound addr", obs_addr.to_string());

    // Seed the app database on the ACTUAL boot DB through the handler.
    let alice = stamped_session(0x31, "alice", [0xAA; 16]);
    let bob = stamped_session(0x32, "bob", [0xBB; 16]);
    let carol = stamped_session(0x33, "carol", [0xCC; 16]);
    let alice_arc = store.insert(alice.session_id, alice);
    let bob_arc = store.insert(bob.session_id, bob);
    let carol_arc = store.insert(carol.session_id, carol);
    assert_ne!(alice_arc.activity_handle(), bob_arc.activity_handle());
    assert_ne!(bob_arc.activity_handle(), carol_arc.activity_handle());
    assert_ne!(alice_arc.activity_handle(), carol_arc.activity_handle());

    let owner = Actor::User(principal64([0xAA; 16]));
    let db = handler.db().clone();
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner.clone(),
    )
    .await
    .expect("add repo");
    let open = shamir_types::access::ResourceMeta::open();
    for path in [
        shamir_types::access::ResourcePath::database("app"),
        shamir_types::access::ResourcePath::store("app", "main"),
        shamir_types::access::ResourcePath::table("app", "main", "items"),
    ] {
        db.set_resource_meta(&path, &open)
            .await
            .expect("open resource");
    }

    // Seed rows through the real handler (released budget: no hog yet)
    // over the REAL public wire path — `RequestHandler::handle`, the same
    // future shape the parked writes below use — not a private method.
    let seed = async {
        let mut b = Batch::new();
        for i in 0..4 {
            b.insert(
                format!("seed{i}"),
                insert("items").row(doc! { "id" => format!("k{i}"), "v" => i }),
            );
        }
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: b.build(),
        };
        let bytes = rmp_serde::to_vec_named(&req).expect("encode seed insert");
        let raw = handler
            .handle(&alice_arc, &bytes, &ConnectionServices::without_push(0))
            .await
            .expect("seed insert handle");
        rmp_serde::from_slice::<DbResponse>(&raw).expect("decode seed insert response")
    };
    let seeded = tokio::time::timeout(CALL_TIMEOUT, seed)
        .await
        .expect("bounded seed insert");
    assert!(
        matches!(seeded, DbResponse::Batch { .. }),
        "seed insert must succeed, got {seeded:?}"
    );
    guard.mark("db + rows seeded");

    // One real interactive transaction, parked in the ACTUAL tx
    // registry of the production handler (opened directly on the boot
    // DB, registered the same way the wire tx path does).
    let (tx, snapshot) = db
        .tx_begin_as(owner, "app", "main", "snapshot")
        .await
        .expect("begin real transaction");
    let tx_handle = tx.tx_id.0;
    handler
        .tx_registry()
        .register(
            tx_handle,
            InteractiveTx::new(
                tx,
                snapshot,
                alice_arc.session_id,
                alice_arc.user_id,
                "app".into(),
                "main".into(),
                Duration::from_secs(3600),
            ),
        )
        .expect("park transaction");
    assert_eq!(handler.activity_counts().transactions, 1);

    // Two real UN-DRAINED cursors through the actual wire
    // `CreateCursor` path (first page returned, never fetched again).
    let mut cursor_ids = Vec::new();
    for i in 0..2 {
        let req = DbRequest::CreateCursor {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            query: Query::from("items").build(),
            page_size: 2,
        };
        let bytes = rmp_serde::to_vec_named(&req).expect("encode CreateCursor");
        let open_cursor = async {
            handler
                .handle(&alice_arc, &bytes, &ConnectionServices::without_push(0))
                .await
        };
        let raw = tokio::time::timeout(CALL_TIMEOUT, open_cursor)
            .await
            .expect("bounded CreateCursor")
            .expect("CreateCursor must succeed");
        let decoded: DbResponse = rmp_serde::from_slice(&raw).expect("decode CursorPage");
        let DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } = decoded
        else {
            panic!("expected CursorPage, got {decoded:?}");
        };
        assert!(
            has_more,
            "cursor must have more pages (4 rows, page_size 2)"
        );
        let CursorId(id) = cursor_id;
        cursor_ids.push(id);
        guard.note("cursor opened", format!("cursor {id} (call {i})"));
    }
    assert_eq!(cursor_ids.len(), 2);
    assert_eq!(handler.activity_counts().cursors, 2);

    // Hold the FULL shared budget: the parked writes' upfront
    // reservations cannot fit the remaining 0 bytes.
    let cap = seam.byte_budget.cap().expect("finite budget configured");
    let hog = seam.byte_budget.acquire(cap).await;
    assert_eq!(seam.byte_budget.used(), cap, "budget fully held");

    // Four ordinary write batches, each polled EXACTLY ONCE to
    // `Pending` (registration happens BEFORE the budget acquire, so
    // all four are deterministically Running while parked), then kept
    // alive without further polls.
    let mut parked: Vec<Pin<Box<dyn Future<Output = DbResponse> + '_>>> = Vec::new();
    for i in 0..4 {
        let mut b = Batch::new();
        b.limits(BatchLimits {
            max_result_size: 4096,
            ..BatchLimits::default()
        });
        b.insert(
            format!("held{i}"),
            insert("items").row(doc! {
                "id" => format!("held-{i}"),
                "v" => i,
                "note" => CANARY,
            }),
        );
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: b.build(),
        };
        let bytes = rmp_serde::to_vec_named(&req).expect("encode held write");
        let handler_ref = &handler;
        let session_ref = &alice_arc;
        let mut fut = Box::pin(async move {
            let raw = handler_ref
                .handle(session_ref, &bytes, &ConnectionServices::without_push(0))
                .await
                .expect("held write handle");
            rmp_serde::from_slice::<DbResponse>(&raw).expect("decode held write response")
        });
        poll_fn(|cx| {
            assert!(
                matches!(fut.as_mut().poll(cx), Poll::Pending),
                "held write {i} must park at the saturated budget, not complete"
            );
            Poll::Ready(())
        })
        .await;
        parked.push(fut);
    }
    assert_eq!(handler.activity_counts().requests, 4, "four parked writes");
    guard.mark("4 writes parked, 3 sessions, 1 tx, 2 cursors live");

    // Mirror cross-check (exact counts the gauges must render).
    let mirror = handler.activity_counts();
    assert_eq!(mirror.requests, 4);
    assert_eq!(mirror.sessions, 3);
    assert_eq!(mirror.transactions, 1);
    assert_eq!(mirror.cursors, 2);

    // THE BINDING PROOF: a real loopback HTTP scrape of the address
    // the observability server ACTUALLY bound renders the four
    // `shamir_active_*` gauges with exactly those counts. Fails if the
    // launcher omitted `set_activity_source` (the still-rendering family
    // drops to all zeros) or bound it to any other handler (counts wrong).
    let metrics = http_get(obs_addr, "/metrics").await;
    guard.note("scrape", "first /metrics fetched");
    assert_eq!(gauge_value(&metrics, "shamir_active_requests"), 4);
    assert_eq!(gauge_value(&metrics, "shamir_active_sessions"), 3);
    assert_eq!(gauge_value(&metrics, "shamir_active_transactions"), 1);
    assert_eq!(gauge_value(&metrics, "shamir_active_cursors"), 2);

    // Release/destroy the held work: dropping the budget permit and
    // the parked futures runs every in-flight RAII guard, so the
    // gauges return to the non-live baseline. The seeded session, tx
    // and cursors are still open — they release only at shutdown.
    drop(hog);
    drop(parked);
    assert_eq!(handler.activity_counts().requests, 0);
    let metrics = http_get(obs_addr, "/metrics").await;
    guard.note("scrape", "post-release /metrics fetched");
    assert_eq!(gauge_value(&metrics, "shamir_active_requests"), 0);
    assert_eq!(gauge_value(&metrics, "shamir_active_sessions"), 3);
    assert_eq!(gauge_value(&metrics, "shamir_active_transactions"), 1);
    assert_eq!(gauge_value(&metrics, "shamir_active_cursors"), 2);

    // Drop THIS test's extra strong references before shutdown: the
    // clones of the boot db, the concrete handler and the session
    // store (plus the seeded session arcs) would otherwise keep the
    // stores alive past the release barrier. The seam itself also
    // holds handler/store strong refs — take it off the handle first.
    drop(db);
    drop(handler);
    drop(store);
    drop(alice_arc);
    drop(bob_arc);
    drop(carol_arc);
    let mut handle = handle;
    drop(handle.activity_test_seam.take());

    // Bounded shutdown releases the registries and the store locks.
    let report = tokio::time::timeout(CALL_TIMEOUT, handle.shutdown())
        .await
        .expect("bounded shutdown");
    guard.mark("shutdown complete");
    assert!(report.is_clean(), "shutdown must be clean: {report:?}");
    drop(temp);
}
