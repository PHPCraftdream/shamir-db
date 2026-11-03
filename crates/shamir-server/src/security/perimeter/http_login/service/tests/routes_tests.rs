use super::fixture::Fixture;
use crate::http_login::admission::{LoginHeadPolicy, Source};
use crate::http_login::service::LoginServiceContext;
use crate::http_login::HttpLoginRoutes;
use bytes::Bytes;
use shamir_transport_http::LoginRoutes;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

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
            admission
                .admit_slot_for_test(source())
                .await
                .expect("permit"),
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
        permit,
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
        permit,
        Bytes::new(),
    ));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(future);

    let started = tokio::time::Instant::now();
    assert!(!routes.shutdown(Duration::from_millis(1)).await);
    assert!(tokio::time::Instant::now() - started >= Duration::from_millis(1));
    assert!(tokio::time::Instant::now() - started < Duration::from_millis(50));
    assert!(routes.shutdown(Duration::from_secs(1)).await);
    assert_eq!(pending.count().await, 0);

    let rejected = routes
        .handle(
            Bytes::from_static(b"/shamir/v1/auth/begin"),
            admission
                .admit_slot_for_test(source())
                .await
                .expect("permit"),
            Bytes::new(),
        )
        .await;
    assert_eq!(
        rejected.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(pending.count().await, 0);
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
        permit,
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
