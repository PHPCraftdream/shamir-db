use super::super::{accept_retry, AcceptRetry};
use std::{future::Future, io, task::Poll, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn transient_accept_errors_retry_without_backoff() {
    let shutdown = CancellationToken::new();
    for kind in [
        io::ErrorKind::ConnectionAborted,
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::Interrupted,
    ] {
        let error = io::Error::from(kind);
        let mut delay = Duration::from_millis(10);
        let before = tokio::time::Instant::now();
        assert_eq!(
            accept_retry(&error, &mut delay, &shutdown).await,
            AcceptRetry::Continue
        );
        assert_eq!(tokio::time::Instant::now(), before);
        assert_eq!(delay, Duration::from_millis(10));
    }
}

#[tokio::test(start_paused = true)]
async fn resource_and_generic_errors_use_increasing_capped_backoff() {
    let shutdown = CancellationToken::new();
    for error in [
        io::Error::from(io::ErrorKind::WouldBlock),
        io::Error::other("unexpected accept failure"),
    ] {
        let mut delay = Duration::from_millis(10);
        for expected in [10, 20, 40, 80, 160, 320, 640, 1000, 1000] {
            let mut retry = Box::pin(accept_retry(&error, &mut delay, &shutdown));
            assert!(
                std::future::poll_fn(|cx| { Poll::Ready(retry.as_mut().poll(cx).is_pending()) })
                    .await
            );
            tokio::time::advance(Duration::from_millis(expected - 1)).await;
            assert!(
                std::future::poll_fn(|cx| { Poll::Ready(retry.as_mut().poll(cx).is_pending()) })
                    .await
            );
            tokio::time::advance(Duration::from_millis(1)).await;
            assert_eq!(retry.await, AcceptRetry::Continue);
            assert_eq!(
                delay,
                Duration::from_millis(expected * 2).min(Duration::from_secs(1))
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_accept_error_backoff() {
    let error = io::Error::other("unexpected accept failure");
    let shutdown = CancellationToken::new();
    let mut delay = Duration::from_secs(1);
    let mut retry = Box::pin(accept_retry(&error, &mut delay, &shutdown));
    assert!(std::future::poll_fn(|cx| { Poll::Ready(retry.as_mut().poll(cx).is_pending()) }).await);
    shutdown.cancel();
    assert_eq!(retry.await, AcceptRetry::Stop);
}
