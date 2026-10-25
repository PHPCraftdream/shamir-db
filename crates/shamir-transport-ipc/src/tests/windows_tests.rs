use std::future::Future;
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

use crate::IpcListener;

/// Unique-enough per-test pipe name — Windows named pipes share one
/// process-wide namespace (`\\.\pipe\...`), so a fixed literal name would
/// collide across tests run in the same nextest process.
fn test_pipe_name(tag: &str) -> String {
    format!(
        r"\\.\pipe\shamir-transport-ipc-test-{tag}-{}",
        std::process::id()
    )
}

#[tokio::test]
async fn accept_and_round_trip_bytes() {
    let name = test_pipe_name("roundtrip");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.expect("accept");
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("write");
    });

    let mut client = ClientOptions::new()
        .open(&client_name)
        .expect("open client");
    client.write_all(b"hello").await.expect("write");
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).await.expect("read");
    assert_eq!(&buf, b"world");

    server.await.expect("server task");
}

#[tokio::test]
async fn listener_serves_a_second_client_after_the_first_disconnects() {
    let name = test_pipe_name("second-client");
    let mut listener = IpcListener::bind(&name).await.expect("bind");
    let client_name = name.clone();

    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let mut stream = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.expect("read");
            assert_eq!(&buf, b"ping");
        }
    });

    for _ in 0..2 {
        let mut client = ClientOptions::new()
            .open(&client_name)
            .expect("open client");
        client.write_all(b"ping").await.expect("write");
        drop(client);
    }

    server.await.expect("server task");
}

/// RV-TRANSPORT-IPC-EFF55124: aborting an in-flight `accept` (after it took
/// the pending instance but before the client connected) must leave the
/// listener usable — the next `accept` must still work instead of panicking
/// on a missing pending instance.
#[tokio::test]
async fn rv_transport_ipc_eff55124_aborted_accept_leaves_listener_usable() {
    let name = test_pipe_name("eff55124");
    let listener = IpcListener::bind(&name).await.expect("bind");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();

    // Server task: poll the accept future exactly once (no client yet →
    // Pending), cancel it by dropping it, signal, then serve a real client
    // with a second accept.
    let server_handle = tokio::spawn(async move {
        let mut listener = listener;
        // Scope the non-Send `Context` so it is dropped before the later
        // `.await` (the spawned future must stay `Send`).
        {
            let mut accept_fut = Box::pin(listener.accept());
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            let poll_result = accept_fut.as_mut().poll(&mut cx);
            assert!(
                matches!(poll_result, Poll::Pending),
                "first accept with no client must be Pending, got {poll_result:?}"
            );
            drop(accept_fut); // the cancellation under test
        }
        ready_tx.send(()).expect("readiness channel still open");

        let mut stream = listener.accept().await.expect("second accept");
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.expect("server read");
        assert_eq!(&buf, b"ping");
    });

    // Open the client only AFTER the server polled once and cancelled —
    // deterministic, no race.
    ready_rx.await.expect("server signalled");
    let mut client = ClientOptions::new().open(&name).expect("open client");
    client.write_all(b"ping").await.expect("client write");

    server_handle.await.expect("server task");
}
