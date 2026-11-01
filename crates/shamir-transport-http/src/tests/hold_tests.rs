use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::Bytes;
use http::{header, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use hyper::{server::conn::http1::Builder, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};

use crate::{
    framing::guarded_io::GuardedIo,
    server::{body::response_body, response::response_held},
};

struct DropSignal {
    count: Arc<AtomicUsize>,
    dropped: Arc<Notify>,
}
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.dropped.notify_one();
    }
}

struct Harness {
    client: tokio::io::DuplexStream,
    server: tokio::task::JoinHandle<Result<(), hyper::Error>>,
    count: Arc<AtomicUsize>,
    dropped: Arc<Notify>,
}

fn start(body: Bytes) -> Harness {
    start_with_write_timeout(body, None)
}

fn start_with_write_timeout(body: Bytes, write_timeout: Option<Duration>) -> Harness {
    const BUFFER_CAPACITY: usize = 64;
    let (client, server_io) = tokio::io::duplex(BUFFER_CAPACITY);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(Notify::new());
    let guarded = GuardedIo::new(server_io, 1024, 64, false, false, hold_rx);
    let guarded = match write_timeout {
        Some(write_timeout) => guarded.with_timeouts(
            Duration::from_secs(10),
            Duration::from_secs(10),
            write_timeout,
        ),
        None => guarded,
    };
    let service_count = count.clone();
    let service_dropped = dropped.clone();
    let service = service_fn(move |_: hyper::Request<hyper::body::Incoming>| {
        let body = body.clone();
        let hold_tx = hold_tx.clone();
        service_count.fetch_add(1, Ordering::SeqCst);
        let hold = Some(Box::new(DropSignal {
            count: service_count.clone(),
            dropped: service_dropped.clone(),
        }) as Box<dyn Send + Sync>);
        async move {
            let response_body = response_body(body, hold, hold_tx).await;
            let mut response = response_held(StatusCode::OK, response_body, false, false, true);
            response
                .headers_mut()
                .insert(header::CONNECTION, http::HeaderValue::from_static("close"));
            Ok::<Response<BoxBody<Bytes, std::convert::Infallible>>, std::convert::Infallible>(
                response,
            )
        }
    });
    let server = tokio::spawn(async move {
        Builder::new()
            .serve_connection(TokioIo::new(guarded), service)
            .await
    });
    Harness {
        client,
        server,
        count,
        dropped,
    }
}

async fn send_request(client: &mut tokio::io::DuplexStream) {
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .await
        .expect("request");
}

async fn read_head<R: AsyncRead + Unpin>(client: &mut R) -> (String, usize) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(client.read_u8().await.expect("response head"));
    }
    let head = String::from_utf8(head).expect("HTTP response head");
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().expect("content length"))
        })
        .expect("content length");
    (head, length)
}

async fn await_drop(dropped: &Notify) {
    tokio::time::timeout(Duration::from_secs(2), dropped.notified())
        .await
        .expect("hold drop notification");
}

#[tokio::test]
async fn response_hold_remains_owned_until_slow_client_reads_full_body() {
    let body = Bytes::from(vec![b'x'; 1024]);
    let mut harness = start(body);
    send_request(&mut harness.client).await;
    let (head, length) =
        tokio::time::timeout(Duration::from_secs(2), read_head(&mut harness.client))
            .await
            .expect("response head");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(length, 1024);
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);
    let mut body = vec![0; length];
    tokio::time::timeout(Duration::from_secs(2), harness.client.read_exact(&mut body))
        .await
        .expect("body read")
        .expect("complete body");
    await_drop(&harness.dropped).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
    harness
        .server
        .await
        .expect("connection task")
        .expect("HTTP connection");
}

#[tokio::test]
async fn disconnect_mid_response_releases_hold_and_finishes_connection() {
    let mut harness = start(Bytes::from(vec![b'x'; 1024]));
    send_request(&mut harness.client).await;
    let (head, _) = tokio::time::timeout(Duration::from_secs(2), read_head(&mut harness.client))
        .await
        .expect("response head");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);
    drop(harness.client);
    await_drop(&harness.dropped).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
    let _connection = tokio::time::timeout(Duration::from_secs(2), harness.server)
        .await
        .expect("connection task completes")
        .expect("connection task");
}

#[tokio::test(start_paused = true)]
async fn write_timeout_releases_hold_when_peer_stops_reading() {
    let write_timeout = Duration::from_secs(1);
    let mut harness = start_with_write_timeout(Bytes::from(vec![b'x'; 1024]), Some(write_timeout));
    send_request(&mut harness.client).await;
    let (head, length) = read_head(&mut harness.client).await;
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(length, 1024);
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);

    tokio::time::advance(write_timeout).await;
    await_drop(&harness.dropped).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
    let connection = harness.server.await.expect("connection task");
    assert!(
        connection.is_err(),
        "write deadline should terminate the stalled response"
    );
}

#[tokio::test]
async fn empty_response_releases_hold_after_flush() {
    let mut harness = start(Bytes::new());
    send_request(&mut harness.client).await;
    let (head, length) =
        tokio::time::timeout(Duration::from_secs(2), read_head(&mut harness.client))
            .await
            .expect("response head");
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(length, 0);
    await_drop(&harness.dropped).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
    harness
        .server
        .await
        .expect("connection task")
        .expect("HTTP connection");
}

// This wrapper is test-only: it observes the real TCP writer's backpressure
// without changing the production GuardedIo or transport API.
struct ObservedTcp {
    stream: tokio::net::TcpStream,
    successful_bytes: Arc<AtomicUsize>,
    write_blocked: Arc<std::sync::atomic::AtomicBool>,
}
impl AsyncRead for ObservedTcp {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl tokio::io::AsyncWrite for ObservedTcp {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.stream).poll_write(cx, buf);
        match &result {
            std::task::Poll::Pending => self.write_blocked.store(true, Ordering::SeqCst),
            std::task::Poll::Ready(Ok(written)) => {
                self.write_blocked.store(false, Ordering::SeqCst);
                self.successful_bytes.fetch_add(*written, Ordering::SeqCst);
            }
            std::task::Poll::Ready(Err(_)) => self.write_blocked.store(false, Ordering::SeqCst),
        }
        result
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

type HttpConnection = Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + Send>>;

struct TcpHarness {
    client: tokio::net::TcpStream,
    connection: HttpConnection,
    count: Arc<AtomicUsize>,
    successful_bytes: Arc<AtomicUsize>,
    write_blocked: Arc<std::sync::atomic::AtomicBool>,
}

async fn start_tcp(body: Bytes, write_timeout: Option<Duration>) -> TcpHarness {
    // Small buffers keep the 16 MiB response blocked on Windows loopback too; the accepted
    // socket inherits the listener's send buffer size.
    let listener_socket = tokio::net::TcpSocket::new_v4().expect("listener socket");
    listener_socket
        .set_send_buffer_size(4096)
        .expect("small send buffer");
    listener_socket
        .bind("127.0.0.1:0".parse().expect("loopback address"))
        .expect("bind loopback");
    let listener = listener_socket.listen(1).expect("listen loopback");
    let address = listener.local_addr().expect("listener address");
    let client_socket = tokio::net::TcpSocket::new_v4().expect("client socket");
    client_socket
        .set_recv_buffer_size(4096)
        .expect("small receive buffer");
    let client = client_socket
        .connect(address)
        .await
        .expect("connect loopback");
    let (stream, _) = listener.accept().await.expect("accept loopback");
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(Notify::new());
    let successful_bytes = Arc::new(AtomicUsize::new(0));
    let write_blocked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = ObservedTcp {
        stream,
        successful_bytes: successful_bytes.clone(),
        write_blocked: write_blocked.clone(),
    };
    let guarded = GuardedIo::new(observed, 1024, 64, false, false, hold_rx);
    let guarded = match write_timeout {
        Some(timeout) => {
            guarded.with_timeouts(Duration::from_secs(10), Duration::from_secs(10), timeout)
        }
        None => guarded,
    };
    let service_count = count.clone();
    let service_dropped = dropped.clone();
    let service = service_fn(move |_: hyper::Request<hyper::body::Incoming>| {
        let body = body.clone();
        let hold_tx = hold_tx.clone();
        service_count.fetch_add(1, Ordering::SeqCst);
        let hold = Some(Box::new(DropSignal {
            count: service_count.clone(),
            dropped: service_dropped.clone(),
        }) as Box<dyn Send + Sync>);
        async move {
            let response_body = response_body(body, hold, hold_tx).await;
            let mut response = response_held(StatusCode::OK, response_body, false, false, true);
            response
                .headers_mut()
                .insert(header::CONNECTION, http::HeaderValue::from_static("close"));
            Ok::<Response<BoxBody<Bytes, std::convert::Infallible>>, std::convert::Infallible>(
                response,
            )
        }
    });
    let connection = Box::pin(Builder::new().serve_connection(TokioIo::new(guarded), service));
    TcpHarness {
        client,
        connection,
        count,
        successful_bytes,
        write_blocked,
    }
}

async fn wait_for_tcp_write_backpressure(harness: &mut TcpHarness, head_size: usize) {
    tokio::time::timeout(
        Duration::from_secs(10),
        std::future::poll_fn(|cx| match harness.connection.as_mut().poll(cx) {
            std::task::Poll::Ready(result) => {
                panic!("TCP connection ended prematurely: {result:?}")
            }
            std::task::Poll::Pending => {
                if harness.successful_bytes.load(Ordering::SeqCst) > head_size
                    && harness.count.load(Ordering::SeqCst) == 1
                    && harness.write_blocked.load(Ordering::SeqCst)
                {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        }),
    )
    .await
    .expect("server TCP poll_write reached Pending after body bytes");
}

async fn tcp_request(client: &mut tokio::net::TcpStream) {
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .await
        .expect("TCP request");
}

#[tokio::test]
async fn tcp_hold_remains_until_blocked_response_is_fully_read() {
    let body = Bytes::from(vec![b'x'; 16 * 1024 * 1024]);
    let mut harness = start_tcp(body, None).await;
    tcp_request(&mut harness.client).await;
    wait_for_tcp_write_backpressure(&mut harness, 1024).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);

    let read_response = async {
        let (head, length) = read_head(&mut harness.client).await;
        assert!(head.starts_with("HTTP/1.1 200"));
        assert_eq!(length, 16 * 1024 * 1024);
        let mut response_body = vec![0; length];
        harness
            .client
            .read_exact(&mut response_body)
            .await
            .expect("complete TCP body");
        let mut extra = [0; 1];
        assert_eq!(harness.client.read(&mut extra).await.expect("TCP EOF"), 0);
        response_body
    };
    let (connection, response_body) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(30), harness.connection.as_mut()),
        tokio::time::timeout(Duration::from_secs(30), read_response),
    );
    connection
        .expect("TCP connection completes")
        .expect("HTTP connection");
    let response_body = response_body.expect("read full TCP body and EOF");
    assert!(response_body.iter().all(|byte| *byte == b'x'));
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tcp_disconnect_releases_hold_after_observed_blocked_write() {
    let mut harness = start_tcp(Bytes::from(vec![b'x'; 16 * 1024 * 1024]), None).await;
    tcp_request(&mut harness.client).await;
    wait_for_tcp_write_backpressure(&mut harness, 1024).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);

    drop(harness.client);
    let _connection = tokio::time::timeout(Duration::from_secs(10), harness.connection.as_mut())
        .await
        .expect("TCP connection completes");
    drop(harness.connection);
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tcp_write_deadline_releases_hold_when_observed_blocked() {
    let timeout = Duration::from_secs(1);
    let mut harness = start_tcp(Bytes::from(vec![b'x'; 16 * 1024 * 1024]), Some(timeout)).await;
    tcp_request(&mut harness.client).await;
    wait_for_tcp_write_backpressure(&mut harness, 1024).await;
    assert_eq!(harness.count.load(Ordering::SeqCst), 1);

    tokio::time::pause();
    tokio::time::advance(timeout).await;
    let connection =
        tokio::time::timeout(Duration::from_secs(5), harness.connection.as_mut()).await;
    assert!(
        matches!(connection, Ok(Err(_))),
        "write deadline ends blocked response: {connection:?}"
    );
    drop(harness.connection);
    assert_eq!(harness.count.load(Ordering::SeqCst), 0);
}
