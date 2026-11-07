use crate::server::body::response_body;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Body, server::conn::http1::Builder, service::service_fn, Response};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    future::{poll_fn, Future},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
};
use tokio::io::{duplex, AsyncRead, AsyncReadExt, AsyncWriteExt};

struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn held_empty_response_transfers_hold_before_end_stream() {
    let dropped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let body = response_body(Bytes::new(), Some(Box::new(DropFlag(dropped.clone()))), tx).await;
    assert!(body.is_end_stream());
    drop(body);
    let (mut client, server) = duplex(4096);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx);
    assert!(!dropped.load(Ordering::SeqCst));
    io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    io.flush().await.unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    drop(io);
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.unwrap();
    assert!(received.starts_with(b"HTTP/1.1 200 OK"));
}

#[tokio::test(start_paused = true)]
async fn dropping_connection_releases_empty_response_hold() {
    let dropped = Arc::new(AtomicBool::new(false));
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let body = response_body(Bytes::new(), Some(Box::new(DropFlag(dropped.clone()))), tx).await;
    assert!(body.is_end_stream());
    drop(body);
    let (_client, server) = duplex(4096);
    let io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx);
    drop(io);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn configured_head_deadline_wakes_parked_read() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(1),
        );
    let mut output = [0; 8];
    poll_fn(|cx| {
        let mut buffer = tokio::io::ReadBuf::new(&mut output);
        match std::pin::Pin::new(&mut io).poll_read(cx, &mut buffer) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("empty peer unexpectedly completed read"),
        }
    })
    .await;
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert_eq!(io.read(&mut output).await.unwrap(), 0);
    drop(io);
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
    assert!(response.ends_with(b"\r\n\r\n"));
}

#[tokio::test(start_paused = true)]
async fn timed_out_partial_response_is_not_followed_by_canned_bytes() {
    let (mut client, server) = duplex(128);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(1),
        );
    io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
        .await
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let result = io.write_all(b"more").await;
    assert!(result.is_err());
    drop(io);
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.unwrap();
    assert!(received.starts_with(b"HTTP/1.1 200 OK"));
    assert!(!received
        .windows(b"408 Request".len())
        .any(|window| window == b"408 Request"));
}

#[tokio::test(start_paused = true)]
async fn idle_flush_does_not_reset_partial_head_deadline() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(1),
        );
    client.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
    let mut output = [0; 8];
    poll_fn(|cx| {
        let mut buffer = tokio::io::ReadBuf::new(&mut output);
        match std::pin::Pin::new(&mut io).poll_read(cx, &mut buffer) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(_) => panic!("incomplete head unexpectedly completed read"),
        }
    })
    .await;
    tokio::time::advance(std::time::Duration::from_millis(700)).await;
    io.flush().await.unwrap();
    tokio::time::advance(std::time::Duration::from_millis(400)).await;
    assert_eq!(io.read(&mut output).await.unwrap(), 0);
    drop(io);
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 408 Request Timeout\r\n"));
}

#[tokio::test(start_paused = true)]
async fn response_head_timeout_does_not_expire_during_response_write() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(10),
        );
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut request = [0; 128];
    let request_len = io.read(&mut request).await.unwrap();
    assert_eq!(
        &request[..request_len],
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"
    );
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    let mut output = [0; 8];
    let mut waiting = Box::pin(io.read(&mut output));
    assert!(poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx).is_pending())).await);
    drop(waiting);
    io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    io.flush().await.unwrap();
    drop(io);
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.unwrap();
    assert_eq!(received, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
}

#[tokio::test(start_paused = true)]
async fn whole_request_deadline_expires_while_waiting_after_head() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(10),
        );
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut request = [0; 128];
    let request_len = io.read(&mut request).await.unwrap();
    assert_eq!(
        &request[..request_len],
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"
    );
    let mut output = [0; 8];
    let mut waiting = Box::pin(io.read(&mut output));
    assert!(poll_fn(|cx| Poll::Ready(waiting.as_mut().poll(cx).is_pending())).await);
    drop(waiting);
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    assert_eq!(io.read(&mut output).await.unwrap(), 0);
    drop(io);
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.unwrap();
    assert_eq!(
        received,
        b"HTTP/1.1 408 Request Timeout\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
}

#[tokio::test(start_paused = true)]
async fn hyper_timeout_drains_canned_rejection_before_close() {
    let (mut client, server) = duplex(32);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let guarded = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(5),
        );
    let called = Arc::new(AtomicBool::new(false));
    let service_called = called.clone();
    let service = service_fn(move |_request| {
        service_called.store(true, Ordering::SeqCst);
        async { Ok::<_, Infallible>(Response::new(Full::new(Bytes::new()))) }
    });
    let mut builder = Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(1));
    let mut connection = Box::pin(builder.serve_connection(TokioIo::new(guarded), service));
    client.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
    assert!(
        poll_fn(|cx| { std::task::Poll::Ready(connection.as_mut().poll(cx).is_pending()) }).await
    );
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let error = poll_fn(|cx| match connection.as_mut().poll(cx) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(Err(error)) => Poll::Ready(error),
        Poll::Ready(Ok(())) => panic!("connection ended without a timeout"),
    })
    .await;
    assert!(error.is_timeout());
    assert!(!called.load(Ordering::SeqCst));
    let connection = *std::pin::Pin::into_inner(connection);
    let mut guarded = connection.into_parts().io.into_inner();
    let mut received = Vec::new();
    let (drained, read) = tokio::join!(
        async move {
            let result = guarded.finish_rejection(error.is_timeout()).await;
            drop(guarded);
            result
        },
        client.read_to_end(&mut received)
    );
    drained.unwrap();
    read.unwrap();
    assert_eq!(
        received,
        b"HTTP/1.1 408 Request Timeout\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
}

#[tokio::test(start_paused = true)]
async fn hyper_request_deadline_expires_while_service_is_pending() {
    let (mut client, server) = duplex(4096);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let guarded = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(1),
        );
    let called = Arc::new(AtomicBool::new(false));
    let service_called = called.clone();
    let service = service_fn(move |_request| {
        service_called.store(true, Ordering::SeqCst);
        std::future::pending::<Result<Response<Full<Bytes>>, Infallible>>()
    });
    let mut builder = Builder::new();
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(10));
    let mut connection = Box::pin(builder.serve_connection(TokioIo::new(guarded), service));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    assert!(
        poll_fn(|cx| { std::task::Poll::Ready(connection.as_mut().poll(cx).is_pending()) }).await
    );
    assert!(called.load(Ordering::SeqCst));
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    assert!(
        poll_fn(|cx| { std::task::Poll::Ready(connection.as_mut().poll(cx).is_pending()) }).await
    );
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let terminal = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        poll_fn(|cx| connection.as_mut().poll(cx)),
    )
    .await
    .expect("request deadline must finish the Hyper connection");
    let timeout_error = terminal
        .as_ref()
        .err()
        .is_some_and(|error| error.is_timeout());
    let connection = *std::pin::Pin::into_inner(connection);
    let mut guarded = connection.into_parts().io.into_inner();
    let mut received = Vec::new();
    let (drained, read) = tokio::join!(
        async move {
            let result = guarded.finish_rejection(timeout_error).await;
            drop(guarded);
            result
        },
        client.read_to_end(&mut received)
    );
    drained.unwrap();
    read.unwrap();
    assert_eq!(
        received,
        b"HTTP/1.1 408 Request Timeout\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: application/msgpack\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
}

#[tokio::test(start_paused = true)]
async fn blocked_first_response_write_uses_write_deadline() {
    let (mut client, server) = duplex(1);
    let (_tx, rx) = tokio::sync::mpsc::channel(1);
    let mut io = super::super::guarded_io::GuardedIo::new(server, 8192, 64, false, false, rx)
        .with_timeouts(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(1),
        );
    assert_eq!(io.write(b"H").await.unwrap(), 1);
    let mut blocked = Box::pin(io.write(b"TTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"));
    assert!(poll_fn(|cx| Poll::Ready(blocked.as_mut().poll(cx).is_pending())).await);
    drop(blocked);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let result = io
        .write(b"TTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
        .await;
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    drop(io);
    let mut received = Vec::new();
    client.read_to_end(&mut received).await.unwrap();
    assert_eq!(received, b"H");
}
