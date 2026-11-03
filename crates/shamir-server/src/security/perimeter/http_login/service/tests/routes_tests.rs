use super::fixture::Fixture;
use crate::http_login::admission::{LoginHeadPolicy, Source};
use crate::http_login::pending::{PendingEntry, PendingId};
use crate::http_login::service::LoginServiceContext;
use crate::http_login::HttpLoginRoutes;
use bytes::Bytes;
use shamir_connect::server::http_login::{build_transcript, HttpLoginTranscriptInput};
use shamir_transport_http::LoginRoutes;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use zeroize::Zeroizing;

struct WorkerDrop(Option<tokio::sync::oneshot::Sender<tokio::time::Instant>>);

impl Drop for WorkerDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            // Best effort: the test receiver may already have disappeared.
            let _ = sender.send(tokio::time::Instant::now());
        }
    }
}

fn source() -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

#[tokio::test]
async fn class_zero_rejects_non_loopback_peer_and_accepts_mapped_loopback_peer() {
    let fixture = Fixture::new().await;
    let routes = routes(fixture.context);
    let headers = axum::http::HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/msgpack".parse().expect("content type"),
        ),
        (
            axum::http::header::HOST,
            "localhost:8080".parse().expect("host"),
        ),
    ]);
    let head = shamir_transport_http::LoginHead::new(b"POST", b"/", &headers);
    let rejected = routes
        .admit_head(
            b"/shamir/v1/auth/begin",
            head,
            shamir_transport_http::PeerInfo {
                addr: "192.0.2.4:88".parse().expect("peer"),
                tls: false,
                forwarded_for: Vec::new(),
            },
        )
        .await
        .err()
        .expect("route rejects peer");
    assert_eq!(rejected.status(), axum::http::StatusCode::FORBIDDEN);
    assert_eq!(
        rejected.body(),
        crate::http_login::wire::response(403, "proxy_forbidden", None)
            .expect("canonical proxy denial")
            .body()
    );

    let headers = axum::http::HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/msgpack".parse().expect("content type"),
        ),
        (
            axum::http::header::HOST,
            "localhost:8080".parse().expect("host"),
        ),
    ]);
    let permit = routes
        .admit_head(
            b"/shamir/v1/auth/begin",
            shamir_transport_http::LoginHead::new(b"POST", b"/", &headers),
            shamir_transport_http::PeerInfo {
                addr: "[::ffff:127.0.0.1]:88".parse().expect("mapped loopback"),
                tls: false,
                forwarded_for: Vec::new(),
            },
        )
        .await
        .expect("mapped loopback admitted");
    drop(permit);
}
#[tokio::test]
async fn global_input_capacity_returns_503_without_touching_pending_state() {
    let fixture = Fixture::new().await;
    let admission = Arc::clone(&fixture.context.admission);
    let pending = Arc::clone(&fixture.context.pending);
    let id = PendingId([71; 32]);
    let listener_id = fixture.context.listener_id;
    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: fixture.context.security_class,
        listener_id: &listener_id,
        handshake_id: &id.0,
        user: "alice",
        client_nonce: &[1; 32],
        server_nonce: &[2; 32],
        salt: &[4; 16],
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
        scope_bytes: &[0, 1, b'x', 0, 0, 0xff],
    })
    .expect("build retained transcript");
    pending
        .insert(
            id,
            PendingEntry {
                transcript: Zeroizing::new(transcript.clone()),
                user_id: None,
                stored_key: Zeroizing::new([1; 32]),
                server_key: Zeroizing::new([2; 32]),
                tickets_invalid_before_ns: 0,
                begin_time_ns: 1,
                eligible: false,
                listener_id,
                security_class: 0,
                source: source(),
            },
        )
        .await
        .expect("insert pending entry");
    assert_eq!(pending.count().await, 1);
    let routes = routes(fixture.context);
    let mut permits = Vec::new();
    for index in 0u16..1_024 {
        let parent = index / 64;
        let within_parent = index % 64;
        let source = Source::from_ip(IpAddr::V4(Ipv4Addr::new(
            10,
            parent as u8,
            (within_parent / 16) as u8,
            (within_parent % 16 + 1) as u8,
        )));
        permits.push(admission.admit_slot_for_test(source).await.expect("slot"));
    }
    let headers = axum::http::HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/msgpack".parse().expect("content type"),
        ),
        (
            axum::http::header::HOST,
            "localhost:8080".parse().expect("host"),
        ),
    ]);
    let response = routes
        .admit_head(
            b"/shamir/v1/auth/begin",
            shamir_transport_http::LoginHead::new(b"POST", b"/", &headers),
            shamir_transport_http::PeerInfo {
                addr: "127.0.0.1:88".parse().expect("peer"),
                tls: false,
                forwarded_for: Vec::new(),
            },
        )
        .await
        .err()
        .expect("route rejects peer");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(response.retry_after(), Some(1));
    assert_eq!(pending.count().await, 1);
    drop(permits);
    let mut claim = pending
        .claim(id, source(), listener_id, 0)
        .await
        .expect("pending entry remains claimable");
    let entry = claim.take().expect("claimed pending entry");
    assert_eq!(entry.transcript.as_slice(), transcript.as_slice());
    drop(entry);
    drop(claim);
    assert_eq!(pending.count().await, 0);
}

#[test]
fn worker_drain_budget_tracks_request_timeout_and_fits_the_outer_deadline() {
    assert_eq!(
        HttpLoginRoutes::worker_drain_budget(1),
        Duration::from_millis(1)
    );
    // Workers plus the 2 s audit drain must fit the server's 10 s listener deadline.
    for timeout_ms in [120_000, u64::MAX] {
        assert_eq!(
            HttpLoginRoutes::worker_drain_budget(timeout_ms),
            Duration::from_secs(7)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_without_workers_is_immediate() {
    let fixture = Fixture::new().await;
    let admission = Arc::clone(&fixture.context.admission);
    let pending = Arc::clone(&fixture.context.pending);
    let routes = routes(fixture.context);

    assert!(routes.shutdown(Duration::ZERO).await);
    assert_eq!(pending.count().await, 0);
    let rejected = routes
        .handle(
            Bytes::from_static(b"/shamir/v1/auth/begin"),
            Arc::new(
                admission
                    .admit_slot_for_test(source())
                    .await
                    .expect("permit"),
            ),
            Bytes::new(),
        )
        .await;
    assert_eq!(
        rejected.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(pending.count().await, 0);
}

fn routes(context: LoginServiceContext) -> HttpLoginRoutes {
    HttpLoginRoutes::new(
        true,
        Arc::new(context),
        LoginHeadPolicy::new(
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            "127.0.0.1:8080".parse().expect("bound"),
        )
        .expect("policy"),
    )
}

#[tokio::test(start_paused = true)]
async fn dropping_routes_does_not_abort_detached_worker() {
    let fixture = Fixture::new().await;
    let admission = Arc::clone(&fixture.context.admission);
    let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(1);
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let guard = WorkerDrop(Some(dropped_tx));
    let mut context = fixture.context;
    context.monotonic_now = Arc::new(move || {
        let _guard = &guard;
        started_tx.try_send(()).expect("worker started once");
        tokio::time::Instant::now()
    });
    let routes = routes(context);
    let permit = admission
        .admit_slot_for_test(source())
        .await
        .expect("permit");
    let mut future = Box::pin(routes.handle(
        Bytes::from_static(b"/shamir/v1/auth/begin"),
        Arc::new(permit),
        Bytes::new(),
    ));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    started_rx.recv().await.expect("worker reached BEGIN");
    let started = tokio::time::Instant::now();
    drop(future);
    drop(routes);

    let dropped_at = dropped_rx
        .await
        .expect("worker context released after padding");
    let elapsed = dropped_at.duration_since(started);
    assert!(elapsed >= Duration::from_millis(50));
    assert!(elapsed <= Duration::from_millis(100));
}

#[tokio::test(start_paused = true)]
async fn shutdown_budget_and_dropped_receiver_leave_worker_running() {
    let fixture = Fixture::new().await;
    let admission = Arc::clone(&fixture.context.admission);
    let pending = Arc::clone(&fixture.context.pending);
    let routes = routes(fixture.context);
    let permit = admission
        .admit_slot_for_test(source())
        .await
        .expect("permit");
    let mut future = Box::pin(routes.handle(
        Bytes::from_static(b"/shamir/v1/auth/begin"),
        Arc::new(permit),
        Bytes::new(),
    ));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(future);

    let started = tokio::time::Instant::now();
    assert!(
        !routes
            .shutdown(HttpLoginRoutes::worker_drain_budget(1))
            .await
    );
    assert!(tokio::time::Instant::now() - started >= Duration::from_millis(1));
    assert!(tokio::time::Instant::now() - started < Duration::from_millis(50));
    assert!(routes.shutdown(Duration::from_secs(1)).await);
    assert_eq!(pending.count().await, 0);

    let rejected = routes
        .handle(
            Bytes::from_static(b"/shamir/v1/auth/begin"),
            Arc::new(
                admission
                    .admit_slot_for_test(source())
                    .await
                    .expect("permit"),
            ),
            Bytes::new(),
        )
        .await;
    assert_eq!(
        rejected.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(pending.count().await, 0);
    // The rejected reply still carries the slot until it is dropped.
    drop(rejected);
    let mut permits = Vec::new();
    for _ in 0..16 {
        permits.push(
            admission
                .admit_slot_for_test(source())
                .await
                .expect("released slot"),
        );
    }
    assert!(routes.shutdown(Duration::ZERO).await);
}

#[tokio::test(start_paused = true)]
async fn simultaneous_shutdown_waiters_both_observe_worker_completion() {
    let fixture = Fixture::new().await;
    let admission = Arc::clone(&fixture.context.admission);
    let pending = Arc::clone(&fixture.context.pending);
    let routes = routes(fixture.context);
    let permit = admission
        .admit_slot_for_test(source())
        .await
        .expect("permit");
    let mut future = Box::pin(routes.handle(
        Bytes::from_static(b"/shamir/v1/auth/begin"),
        Arc::new(permit),
        Bytes::new(),
    ));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(future);

    let (first, second) = tokio::join!(
        routes.shutdown(Duration::from_secs(1)),
        routes.shutdown(Duration::from_secs(1))
    );
    assert!(first);
    assert!(second);
    assert_eq!(pending.count().await, 0);
}
